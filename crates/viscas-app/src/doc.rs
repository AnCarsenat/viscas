//! The document side of the app: the shared [`Engine`] handle, the event
//! hub editors subscribe to, and the single pump that drives the engine.
//!
//! - [`EngineHandle`] wraps `Rc<RefCell<Engine>>`. [`EngineHandle::with`]
//!   and [`EngineHandle::edit`] scope every borrow to a closure, so no
//!   borrow is ever held across GTK calls or signal emissions. `edit`
//!   wakes the pump.
//! - The pump runs on idle right after a wake and then every
//!   [`PUMP_INTERVAL`] while background work is pending (plots, features);
//!   it stops when the engine is idle. Each tick calls `update()`, drains
//!   scene events, polls plots and features, prunes the selection and then
//!   broadcasts [`DocEvent`]s through [`DocEvents`] — after every engine
//!   borrow is released.
//! - The modified flag follows the undo history: undoing back to the saved
//!   state makes the document clean again ([`track_saved`]). Layout-only
//!   changes (areas, workspace) never dirty the document; they are stored
//!   with the next save.
//! - Selection and hover live in the app ([`crate::ctx::Selection`]), the
//!   single source of truth; the scene's own selection is not used. Node
//!   removal prunes them and a document replacement clears them.
//! - [`settings_from_prefs`] and [`palette_for_theme`] map preferences to
//!   the engine (pure, tested).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::glib;
use viscas_engine::{DEFAULT_PALETTE, Engine, EngineSettings, FeaturesResult, ViewportId};
use viscas_scene::{Change, Color as SceneColor, NodeId, SceneEvent};

use crate::prefs::{self, Preferences};
use crate::theme::model::Theme;

/// Pump period while background work is pending (~60 Hz).
pub const PUMP_INTERVAL: Duration = Duration::from_millis(16);

/// A notification broadcast by the pump (or by document operations).
#[derive(Clone, Debug)]
#[allow(dead_code, reason = "payloads are read by the editors subscribing")]
pub enum DocEvent {
    /// Scene changes of one pump tick, in mutation order.
    Scene(Rc<[SceneEvent]>),
    /// These viewports received fresh plot layers: redraw them.
    PlotsReady(Rc<[ViewportId]>),
    /// These viewports have missing or outdated layers: request plots.
    Replot(Rc<[ViewportId]>),
    /// A feature-analysis request finished.
    Features(Rc<FeaturesResult>),
    /// Evaluation settings changed (status-bar chips, console `:mode`,
    /// preferences).
    Settings(EngineSettings),
    /// The node colors changed (palette swap): redraw.
    Colors,
    /// Path or modified state of the document changed.
    Document,
    /// A different document was loaded (new/open): editors drop their
    /// per-document state. Followed by `Scene` and `Document` events.
    Replaced,
}

type Listener = Rc<dyn Fn(&DocEvent)>;

/// Observable hub of [`DocEvent`]s.
///
/// [`DocEvents::subscribe`] returns a [`Subscription`] guard; dropping it
/// disconnects. Listeners are cloned before a broadcast, so they may
/// subscribe or unsubscribe while it runs. Listeners must capture widgets
/// and handles weakly.
#[derive(Default)]
pub struct DocEvents {
    listeners: RefCell<Vec<(u64, Listener)>>,
    next: Cell<u64>,
}

/// Keeps a [`DocEvents`] listener connected while alive.
#[must_use = "dropping the subscription disconnects the listener"]
pub struct Subscription {
    hub: Weak<DocEvents>,
    id: u64,
}

impl Subscription {
    /// Keeps the listener connected for the hub's whole lifetime.
    #[allow(dead_code, reason = "for app-lifetime listeners")]
    pub fn forever(self) {
        std::mem::forget(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.listeners.borrow_mut().retain(|(id, _)| *id != self.id);
        }
    }
}

impl DocEvents {
    pub fn subscribe(self: &Rc<Self>, f: impl Fn(&DocEvent) + 'static) -> Subscription {
        let id = self.next.get();
        self.next.set(id + 1);
        self.listeners.borrow_mut().push((id, Rc::new(f)));
        Subscription {
            hub: Rc::downgrade(self),
            id,
        }
    }

    pub fn emit(&self, event: &DocEvent) {
        let listeners: Vec<Listener> = self
            .listeners
            .borrow()
            .iter()
            .map(|(_, f)| f.clone())
            .collect();
        for f in listeners {
            f(event);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.listeners.borrow().len()
    }
}

/// Does this scene event change the saved document?
pub fn modifies_document(event: &SceneEvent) -> bool {
    match event {
        SceneEvent::NodeAdded(_) | SceneEvent::NodeRemoved(_) | SceneEvent::Moved { .. } => true,
        SceneEvent::Changed { what, .. } => *what != Change::Status,
        SceneEvent::SelectionChanged
        | SceneEvent::GraphChanged { .. }
        | SceneEvent::HistoryChanged => false,
    }
}

/// Engine settings for `prefs`, keeping the session modes (exact/approx,
/// angle unit, complex: status-bar chips and console `:mode`) of `current`.
pub fn settings_from_prefs(prefs: &Preferences, current: &EngineSettings) -> EngineSettings {
    EngineSettings {
        definition_op: match prefs.editing.definition_op {
            prefs::DefinitionOp::ColonEquals => viscas_core::syntax::DefinitionOp::ColonEq,
            prefs::DefinitionOp::Equals => viscas_core::syntax::DefinitionOp::Eq,
        },
        ..*current
    }
}

/// Plot color cycle for a theme: the engine palette on dark themes, a
/// darkened copy on light ones so curves keep contrast on white.
pub fn palette_for_theme(theme: &Theme) -> Vec<SceneColor> {
    if theme.dark {
        return DEFAULT_PALETTE.to_vec();
    }
    let darken = |v: u8| (f64::from(v) * 0.72).round() as u8;
    DEFAULT_PALETTE
        .iter()
        .map(|c| SceneColor::rgba(darken(c.r), darken(c.g), darken(c.b), c.a))
        .collect()
}

/// Display name of a document path (`untitled` without one).
pub fn document_name(path: Option<&std::path::Path>) -> String {
    path.and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_owned())
}

/// Window title for a document.
pub fn window_title(path: Option<&std::path::Path>, modified: bool) -> String {
    let dot = if modified { " ●" } else { "" };
    format!("{}{dot} — Viscas", document_name(path))
}

/// Where the saved state sits in the undo history after one pump tick.
///
/// `saved` is the undo depth at the last save/open/new (`None`: the saved
/// state is unreachable, so the document stays modified until saved).
/// `prev_depth`/`depth` are the undo depths before and after the tick,
/// `navigated` says the change came from undo/redo, `edited` that the tick
/// had document-changing events and `full` that the history is at its
/// depth limit (its oldest step was dropped).
pub fn track_saved(
    saved: Option<usize>,
    prev_depth: usize,
    depth: usize,
    navigated: bool,
    edited: bool,
    full: bool,
) -> Option<usize> {
    let saved = saved?;
    if navigated || depth < prev_depth || !edited {
        return Some(saved);
    }
    // A new edit: it replaces the redo branch, so a saved point that was
    // only reachable by redo is gone; an edit that did not grow the
    // history (truncated at the limit) shifts or drops the saved point.
    if prev_depth < saved {
        return None;
    }
    if depth == prev_depth && (full || prev_depth == saved) {
        return None;
    }
    Some(saved)
}

/// Hook run on a tick's scene events before they are broadcast.
type EventHook = Rc<dyn Fn(&[SceneEvent])>;

struct Inner {
    engine: RefCell<Engine>,
    events: Rc<DocEvents>,
    /// A tick is queued (idle or timeout).
    scheduled: Cell<bool>,
    /// A tick is running (re-entrant flushes are ignored).
    ticking: Cell<bool>,
    /// Latest feature ticket not delivered yet.
    feature_pending: Cell<Option<u64>>,
    modified: Cell<bool>,
    /// Undo depth of the saved state (see [`track_saved`]).
    saved_depth: Cell<Option<usize>>,
    /// Undo depth seen by the last tick.
    last_depth: Cell<usize>,
    /// Undo/redo ran since the last tick.
    navigated: Cell<bool>,
    last_settings: Cell<EngineSettings>,
    /// Settings to apply on the next tick (status-bar chips).
    pending_settings: Cell<Option<EngineSettings>>,
    /// Viewports whose close found the engine borrowed.
    pending_close: RefCell<Vec<ViewportId>>,
    /// Undo depth limit from the preferences (re-applied to new scenes).
    history_limit: Cell<Option<usize>>,
    /// Run during each tick before broadcasting (selection pruning).
    pre_broadcast: RefCell<Option<EventHook>>,
}

/// Shared handle to the document engine. Cloning is cheap.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Rc<Inner>,
}

impl Default for EngineHandle {
    fn default() -> Self {
        Self::new(Engine::new())
    }
}

impl EngineHandle {
    pub fn new(engine: Engine) -> Self {
        let settings = *engine.settings();
        Self {
            inner: Rc::new(Inner {
                engine: RefCell::new(engine),
                events: Rc::default(),
                scheduled: Cell::new(false),
                ticking: Cell::new(false),
                feature_pending: Cell::new(None),
                modified: Cell::new(false),
                saved_depth: Cell::new(Some(0)),
                last_depth: Cell::new(0),
                navigated: Cell::new(false),
                last_settings: Cell::new(settings),
                pending_settings: Cell::new(None),
                pending_close: RefCell::default(),
                history_limit: Cell::new(None),
                pre_broadcast: RefCell::new(None),
            }),
        }
    }

    /// The event hub.
    pub fn events(&self) -> &Rc<DocEvents> {
        &self.inner.events
    }

    /// Shorthand for `events().subscribe(f)`.
    pub fn subscribe(&self, f: impl Fn(&DocEvent) + 'static) -> Subscription {
        self.inner.events.subscribe(f)
    }

    /// Reads the engine. Do not call GTK or emit signals inside `f`.
    pub fn with<R>(&self, f: impl FnOnce(&Engine) -> R) -> R {
        f(&self.inner.engine.borrow())
    }

    /// Mutates the engine and wakes the pump, which broadcasts the
    /// resulting events. Do not call GTK or emit signals inside `f`. Use
    /// [`EngineHandle::undo`]/[`EngineHandle::redo`] for history
    /// navigation so the saved point is tracked exactly.
    pub fn edit<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        let r = f(&mut self.inner.engine.borrow_mut());
        self.wake();
        r
    }

    /// Undo one step.
    pub fn undo(&self) -> bool {
        self.inner.navigated.set(true);
        self.edit(|e| e.undo())
    }

    /// Redo one step.
    pub fn redo(&self) -> bool {
        self.inner.navigated.set(true);
        self.edit(|e| e.redo())
    }

    /// Applies evaluation settings on the next pump tick (not inside the
    /// caller's event handler). The latest call wins.
    pub fn set_settings_deferred(&self, settings: EngineSettings) {
        self.inner.pending_settings.set(Some(settings));
        self.wake();
    }

    /// The settings that will be in effect after pending changes.
    pub fn effective_settings(&self) -> EngineSettings {
        self.inner
            .pending_settings
            .get()
            .unwrap_or_else(|| self.with(|e| *e.settings()))
    }

    /// Requests features near a world point (see
    /// [`Engine::request_features`]); the result arrives as
    /// [`DocEvent::Features`].
    #[allow(dead_code, reason = "used by viewport feature picking")]
    pub fn request_features(
        &self,
        viewport: ViewportId,
        point: viscas_plot::Vec2,
        radius: f64,
    ) -> Option<u64> {
        let ticket = self.edit(|e| e.request_features(viewport, point, radius));
        if ticket.is_some() {
            self.inner.feature_pending.set(ticket);
        }
        ticket
    }

    /// Allocates an engine viewport for a graph editor.
    #[allow(dead_code, reason = "used by viewport editors")]
    pub fn create_viewport(&self) -> ViewportId {
        self.inner.engine.borrow_mut().create_viewport()
    }

    /// Cancels and drops an engine viewport (editor destroyed). When the
    /// engine is borrowed (an editor disposed during a tick) the close is
    /// queued and done by the next tick.
    #[allow(dead_code, reason = "used by viewport editors")]
    pub fn close_viewport(&self, id: ViewportId) {
        match self.inner.engine.try_borrow_mut() {
            Ok(mut e) => e.close_viewport(id),
            Err(_) => {
                self.inner.pending_close.borrow_mut().push(id);
                self.wake();
            }
        }
    }

    /// Starts plotting `viewport` at `view` and keeps the pump running
    /// until the layers arrive.
    #[allow(dead_code, reason = "used by viewport editors")]
    pub fn request_plots(&self, viewport: ViewportId, view: viscas_plot::View) -> usize {
        self.edit(|e| e.request_plots(viewport, view))
    }

    /// Whether the document differs from its saved state. Pending events
    /// are counted only after a tick; call [`EngineHandle::flush`] first
    /// when the answer must be exact (save prompts).
    pub fn is_modified(&self) -> bool {
        self.inner.modified.get()
    }

    fn set_modified(&self, modified: bool) {
        if self.inner.modified.replace(modified) != modified {
            self.inner.events.emit(&DocEvent::Document);
        }
    }

    /// Starts a save: seals the undo history (the next edit becomes a new
    /// step, so the saved point stays addressable by depth) and returns
    /// the depth being saved. Call [`EngineHandle::flush`] first.
    pub fn begin_save(&self) -> usize {
        let mut e = self.inner.engine.borrow_mut();
        e.scene_mut().seal_history();
        e.scene().undo_depth()
    }

    /// Records the state of [`EngineHandle::begin_save`] as saved (edits
    /// made while the file was written keep the document modified) and
    /// emits [`DocEvent::Document`].
    pub fn finish_save(&self, depth: usize) {
        self.flush();
        self.inner.saved_depth.set(Some(depth));
        let current = self.with(|e| e.scene().undo_depth());
        self.inner.modified.set(current != depth);
        self.inner.events.emit(&DocEvent::Document);
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.with(|e| e.document_path().map(|p| p.to_path_buf()))
    }

    pub fn title(&self) -> String {
        window_title(self.path().as_deref(), self.is_modified())
    }

    /// Hook run on each tick's scene events before they are broadcast.
    pub fn set_pre_broadcast(&self, f: impl Fn(&[SceneEvent]) + 'static) {
        *self.inner.pre_broadcast.borrow_mut() = Some(Rc::new(f));
    }

    /// Schedules a pump tick on idle (no-op when one is queued).
    pub fn wake(&self) {
        if self.inner.scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(&self.inner);
        glib::idle_add_local_once(move || {
            if let Some(inner) = weak.upgrade() {
                let h = EngineHandle { inner };
                h.inner.scheduled.set(false);
                h.tick();
            }
        });
    }

    /// Runs a tick now, so the modified flag and listeners are up to date
    /// (before save prompts and saves). No-op inside a tick.
    pub fn flush(&self) {
        self.tick();
    }

    /// One pump step; reschedules itself while work is pending.
    pub fn tick(&self) {
        if self.inner.ticking.replace(true) {
            return;
        }
        let closes: Vec<ViewportId> = self.inner.pending_close.borrow_mut().drain(..).collect();
        let pending_settings = self.inner.pending_settings.take();
        let (events, ready, replot, features, settings, plotting, depth, full) = {
            let mut e = self.inner.engine.borrow_mut();
            for id in closes {
                e.close_viewport(id);
            }
            if let Some(s) = pending_settings {
                e.set_settings(s);
            }
            e.update();
            let events = e.drain_events();
            let ready = e.poll_plots();
            let features = e.poll_features();
            // Layers can only go missing after scene changes or arriving
            // results; skip the scan otherwise.
            let replot = if events.is_empty() && ready.is_empty() {
                Vec::new()
            } else {
                e.needs_replot()
            };
            let scene = e.scene();
            let depth = scene.undo_depth();
            let limit = self.history_limit_hint();
            (
                events,
                ready,
                replot,
                features,
                *e.settings(),
                e.has_pending_work(),
                depth,
                limit.is_some_and(|l| depth >= l),
            )
        };
        self.inner.ticking.set(false);
        let delivered = features
            .iter()
            .any(|f| Some(f.ticket) == self.inner.feature_pending.get());
        if delivered {
            self.inner.feature_pending.set(None);
        }
        let pending = plotting || self.inner.feature_pending.get().is_some();

        let edited = events.iter().any(modifies_document);
        let saved = track_saved(
            self.inner.saved_depth.get(),
            self.inner.last_depth.replace(depth),
            depth,
            self.inner.navigated.take(),
            edited,
            full,
        );
        self.inner.saved_depth.set(saved);

        if !events.is_empty() {
            let hook = self.inner.pre_broadcast.borrow().clone();
            if let Some(hook) = hook {
                hook(&events);
            }
        }
        let hub = self.inner.events.clone();
        if settings != self.inner.last_settings.replace(settings) {
            hub.emit(&DocEvent::Settings(settings));
        }
        if !events.is_empty() {
            hub.emit(&DocEvent::Scene(events.into()));
        }
        self.set_modified(saved != Some(depth));
        if !ready.is_empty() {
            hub.emit(&DocEvent::PlotsReady(ready.into()));
        }
        for f in features {
            hub.emit(&DocEvent::Features(Rc::new(f)));
        }
        if !replot.is_empty() {
            hub.emit(&DocEvent::Replot(replot.into()));
        }
        if pending && !self.inner.scheduled.replace(true) {
            let weak = Rc::downgrade(&self.inner);
            glib::timeout_add_local_once(PUMP_INTERVAL, move || {
                if let Some(inner) = weak.upgrade() {
                    let h = EngineHandle { inner };
                    h.inner.scheduled.set(false);
                    h.tick();
                }
            });
        }
    }

    /// The undo depth limit, as set by [`EngineHandle::set_history_limit`].
    fn history_limit_hint(&self) -> Option<usize> {
        self.inner.history_limit.get()
    }

    /// Sets the scene's undo depth limit (kept across new/open).
    pub fn set_history_limit(&self, limit: usize) {
        self.inner.history_limit.set(Some(limit));
        self.inner
            .engine
            .borrow_mut()
            .scene_mut()
            .set_history_limit(limit);
    }

    /// Replaces the document with an already loaded one (new/open after
    /// the user confirmed and the file was read): flushes the old
    /// document's events, installs the new one, resets the saved point
    /// and broadcasts [`DocEvent::Replaced`] and [`DocEvent::Document`].
    pub fn replace_with(&self, f: impl FnOnce(&mut Engine)) {
        self.flush();
        {
            let mut e = self.inner.engine.borrow_mut();
            f(&mut e);
            if let Some(limit) = self.inner.history_limit.get() {
                e.scene_mut().set_history_limit(limit);
            }
        }
        self.inner.modified.set(false);
        self.inner.saved_depth.set(Some(0));
        self.inner.last_depth.set(0);
        self.inner.navigated.set(false);
        self.inner.feature_pending.set(None);
        self.inner.events.emit(&DocEvent::Replaced);
        self.inner.events.emit(&DocEvent::Document);
        self.wake();
    }
}

/// Selection ids to keep after the scene events of a tick.
pub fn prune_removed(selected: &[NodeId], events: &[SceneEvent]) -> Option<Vec<NodeId>> {
    let removed: Vec<NodeId> = events
        .iter()
        .filter_map(|e| match e {
            SceneEvent::NodeRemoved(id) => Some(*id),
            _ => None,
        })
        .collect();
    if removed.is_empty() || !selected.iter().any(|id| removed.contains(id)) {
        return None;
    }
    Some(
        selected
            .iter()
            .copied()
            .filter(|id| !removed.contains(id))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::model;

    #[test]
    fn hub_broadcasts_and_guard_disconnects() {
        let hub = Rc::new(DocEvents::default());
        let count = Rc::new(Cell::new(0));
        let c = count.clone();
        let sub = hub.subscribe(move |_| c.set(c.get() + 1));
        hub.emit(&DocEvent::Document);
        assert_eq!(count.get(), 1);
        drop(sub);
        hub.emit(&DocEvent::Document);
        assert_eq!(count.get(), 1);
        assert_eq!(hub.len(), 0);
    }

    #[test]
    fn listeners_may_subscribe_and_unsubscribe_during_broadcast() {
        let hub = Rc::new(DocEvents::default());
        let held: Rc<RefCell<Vec<Subscription>>> = Rc::default();
        let weak_hub = Rc::downgrade(&hub);
        let h = held.clone();
        hub.subscribe(move |_| {
            if let Some(hub) = weak_hub.upgrade() {
                h.borrow_mut().push(hub.subscribe(|_| {}));
            }
        })
        .forever();
        let h = held.clone();
        hub.subscribe(move |_| h.borrow_mut().clear()).forever();
        hub.emit(&DocEvent::Colors);
        assert_eq!(hub.len(), 2, "the new subscription was dropped again");
    }

    #[test]
    fn prefs_map_definition_op_and_keep_session_modes() {
        let mut prefs = Preferences::default();
        let current = EngineSettings {
            approx: true,
            angle: viscas_core::eval::AngleUnit::Degrees,
            ..EngineSettings::default()
        };
        let s = settings_from_prefs(&prefs, &current);
        assert_eq!(s.definition_op, viscas_core::syntax::DefinitionOp::ColonEq);
        assert!(s.approx);
        assert_eq!(s.angle, viscas_core::eval::AngleUnit::Degrees);
        prefs.editing.definition_op = prefs::DefinitionOp::Equals;
        let s = settings_from_prefs(&prefs, &current);
        assert_eq!(s.definition_op, viscas_core::syntax::DefinitionOp::Eq);
    }

    #[test]
    fn light_themes_get_a_darker_palette() {
        let dark = palette_for_theme(&model::viscas_dark());
        let light = palette_for_theme(&model::viscas_light());
        assert_eq!(dark, DEFAULT_PALETTE.to_vec());
        assert_eq!(light.len(), dark.len());
        for (l, d) in light.iter().zip(&dark) {
            assert!(
                u32::from(l.r) + u32::from(l.g) + u32::from(l.b)
                    < u32::from(d.r) + u32::from(d.g) + u32::from(d.b)
            );
        }
    }

    #[test]
    fn titles_and_names() {
        assert_eq!(window_title(None, false), "untitled — Viscas");
        let p = std::path::Path::new("/tmp/curves.viscas");
        assert_eq!(window_title(Some(p), true), "curves ● — Viscas");
    }

    #[test]
    fn saved_point_follows_undo_history() {
        // Saved at depth 2; edit → 3 (dirty), undo → 2 (clean).
        assert_eq!(track_saved(Some(2), 2, 3, false, true, false), Some(2));
        assert_eq!(track_saved(Some(2), 3, 2, true, true, false), Some(2));
        // Undo below the saved point, then a new edit: branch lost.
        assert_eq!(track_saved(Some(2), 1, 2, false, true, false), None);
        // Redo back to it is fine.
        assert_eq!(track_saved(Some(2), 1, 2, true, true, false), Some(2));
        // An edit that did not grow a full history (oldest step dropped).
        assert_eq!(track_saved(Some(2), 5, 5, false, true, true), None);
        assert_eq!(track_saved(Some(5), 5, 5, false, true, false), None);
        // Nothing edited: unchanged; unreachable stays unreachable.
        assert_eq!(track_saved(Some(2), 3, 3, false, false, true), Some(2));
        assert_eq!(track_saved(None, 2, 2, true, true, false), None);
    }

    #[test]
    fn modification_and_pruning() {
        let a = NodeId(1);
        let b = NodeId(2);
        assert!(modifies_document(&SceneEvent::NodeAdded(a)));
        assert!(!modifies_document(&SceneEvent::Changed {
            id: a,
            what: Change::Status
        }));
        assert!(!modifies_document(&SceneEvent::HistoryChanged));
        let events = [SceneEvent::NodeRemoved(a)];
        assert_eq!(prune_removed(&[a, b], &events), Some(vec![b]));
        assert_eq!(prune_removed(&[b], &events), None);
    }

    /// Editors call `edit` from GTK handlers and rely on listeners running
    /// later, in the idle tick (never re-entrantly inside `edit`).
    #[test]
    fn edit_dispatches_on_the_idle_tick_not_synchronously() {
        let main = glib::MainContext::default();
        let _owner = main.acquire().expect("default main context");
        let handle = EngineHandle::new(Engine::with_workers(1));
        let seen = Rc::new(Cell::new(0));
        let s = seen.clone();
        let _sub = handle.subscribe(move |ev| {
            if matches!(ev, DocEvent::Scene(_)) {
                s.set(s.get() + 1);
            }
        });
        handle.edit(|e| {
            e.add_line(None, "a := 1").unwrap();
        });
        assert_eq!(seen.get(), 0, "edit must not broadcast synchronously");
        while main.iteration(false) {}
        assert_eq!(seen.get(), 1, "the idle tick broadcasts once");
    }
}
