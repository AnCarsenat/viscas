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

/// Hook run on a tick's scene events before they are broadcast.
type EventHook = Rc<dyn Fn(&[SceneEvent])>;

struct Inner {
    engine: RefCell<Engine>,
    events: Rc<DocEvents>,
    /// A tick is queued (idle or timeout).
    scheduled: Cell<bool>,
    /// Latest feature ticket not delivered yet.
    feature_pending: Cell<Option<u64>>,
    modified: Cell<bool>,
    last_settings: Cell<EngineSettings>,
    /// Viewports created through the handle (polled while rendering).
    viewports: RefCell<Vec<ViewportId>>,
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
                feature_pending: Cell::new(None),
                modified: Cell::new(false),
                last_settings: Cell::new(settings),
                viewports: RefCell::default(),
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
    /// resulting events. Do not call GTK or emit signals inside `f`.
    pub fn edit<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        let r = f(&mut self.inner.engine.borrow_mut());
        self.wake();
        r
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
        let id = self.inner.engine.borrow_mut().create_viewport();
        self.inner.viewports.borrow_mut().push(id);
        id
    }

    /// Cancels and drops an engine viewport (editor destroyed).
    #[allow(dead_code, reason = "used by viewport editors")]
    pub fn close_viewport(&self, id: ViewportId) {
        self.inner.viewports.borrow_mut().retain(|v| *v != id);
        // The editor may be disposed while the engine is borrowed (e.g. a
        // window closing during a tick): never panic here.
        if let Ok(mut e) = self.inner.engine.try_borrow_mut() {
            e.close_viewport(id);
        }
    }

    /// Starts plotting `viewport` at `view` and keeps the pump running
    /// until the layers arrive.
    #[allow(dead_code, reason = "used by viewport editors")]
    pub fn request_plots(&self, viewport: ViewportId, view: viscas_plot::View) -> usize {
        self.edit(|e| e.request_plots(viewport, view))
    }

    pub fn is_modified(&self) -> bool {
        self.inner.modified.get()
    }

    /// Sets the modified flag; emits [`DocEvent::Document`] on change.
    pub fn set_modified(&self, modified: bool) {
        if self.inner.modified.replace(modified) != modified {
            self.inner.events.emit(&DocEvent::Document);
        }
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
                EngineHandle { inner }.tick();
            }
        });
    }

    /// One pump step; reschedules itself while work is pending.
    pub fn tick(&self) {
        self.inner.scheduled.set(false);
        let (events, ready, replot, features, settings, pending) = {
            let mut e = self.inner.engine.borrow_mut();
            e.update();
            let events = e.drain_events();
            let ready = e.poll_plots();
            let features = e.poll_features();
            let replot = e.needs_replot();
            let settings = *e.settings();
            let plotting = e.queued_tasks() > 0
                || self
                    .inner
                    .viewports
                    .borrow()
                    .iter()
                    .any(|v| e.pending_plots(*v) > 0);
            (events, ready, replot, features, settings, plotting)
        };
        let delivered = features
            .iter()
            .any(|f| Some(f.ticket) == self.inner.feature_pending.get());
        if delivered {
            self.inner.feature_pending.set(None);
        }
        let pending = pending || self.inner.feature_pending.get().is_some();

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
            let modifies = events.iter().any(modifies_document);
            hub.emit(&DocEvent::Scene(events.into()));
            if modifies {
                self.set_modified(true);
            }
        }
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
                    EngineHandle { inner }.tick();
                }
            });
        }
    }

    /// Replaces the document (new/open) and broadcasts [`DocEvent::Replaced`].
    pub fn replace_with<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        let r = f(&mut self.inner.engine.borrow_mut());
        self.inner.modified.set(false);
        self.inner.feature_pending.set(None);
        self.inner.events.emit(&DocEvent::Replaced);
        self.inner.events.emit(&DocEvent::Document);
        self.wake();
        r
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
}
