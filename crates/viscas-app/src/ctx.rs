//! Application context shared by the window, areas, editors and commands.
//!
//! [`AppCtx`] is created once in `app.rs` and passed down; cloning is cheap.
//! It holds application state that is *not* layout: observable preferences
//! (see [`PrefsStore`]), the document engine and its event hub (see
//! [`crate::doc`]), the shared selection/hover model, and the command
//! providers the palette queries.
//! The area layout itself stays pure UI state owned by `Areas`.
//!
//! Listeners owned by a widget or editor are registered with a
//! [`WeakGuard`] (`connect_*_for`): they are dropped as soon as the guard is
//! dead — on the next notification *or* the next registration — so area
//! churn cannot pile up dead listeners.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::glib;
pub use viscas_scene::NodeId;

use crate::commands::{Command, CommandProvider};
use crate::doc::EngineHandle;
use crate::prefs::Preferences;
use crate::theme::model::{self, Theme};

/// A weak handle that a listener is bound to (an editor's `Weak<Inner>`,
/// a widget's `WeakRef`). The listener gets the upgraded handle and is
/// disconnected once the handle is dead.
pub trait WeakGuard: 'static {
    type Strong;
    fn upgrade_guard(&self) -> Option<Self::Strong>;
    /// Whether the handle still upgrades (may be cheaper than upgrading).
    fn is_alive(&self) -> bool {
        self.upgrade_guard().is_some()
    }
}

impl<T: 'static> WeakGuard for Weak<T> {
    type Strong = Rc<T>;
    fn upgrade_guard(&self) -> Option<Rc<T>> {
        self.upgrade()
    }
    fn is_alive(&self) -> bool {
        self.strong_count() > 0
    }
}

impl<T: glib::object::ObjectType> WeakGuard for glib::WeakRef<T> {
    type Strong = T;
    fn upgrade_guard(&self) -> Option<T> {
        self.upgrade()
    }
}

/// One listener: `f` returns `false` to disconnect; `alive` (guarded
/// listeners) tells whether it is still wanted without calling it.
struct Listener<A: ?Sized> {
    alive: Option<Rc<dyn Fn() -> bool>>,
    f: Rc<dyn Fn(&A) -> bool>,
}

impl<A: ?Sized> Clone for Listener<A> {
    fn clone(&self) -> Self {
        Self {
            alive: self.alive.clone(),
            f: self.f.clone(),
        }
    }
}

impl<A: ?Sized> Listener<A> {
    fn is_alive(&self) -> bool {
        self.alive.as_ref().is_none_or(|a| a())
    }
}

/// Listeners taking `&A`. Dead guarded listeners are pruned on every
/// registration and notification.
struct Listeners<A: ?Sized> {
    list: RefCell<Vec<Listener<A>>>,
}

impl<A: ?Sized> Default for Listeners<A> {
    fn default() -> Self {
        Self {
            list: RefCell::default(),
        }
    }
}

impl<A: ?Sized + 'static> Listeners<A> {
    fn push(&self, l: Listener<A>) {
        let mut list = self.list.borrow_mut();
        list.retain(Listener::is_alive);
        list.push(l);
    }

    fn connect(&self, f: impl Fn(&A) -> bool + 'static) {
        self.push(Listener {
            alive: None,
            f: Rc::new(f),
        });
    }

    fn connect_for<G: WeakGuard>(&self, guard: G, f: impl Fn(G::Strong, &A) + 'static) {
        let guard = Rc::new(guard);
        let g = guard.clone();
        self.push(Listener {
            alive: Some(Rc::new(move || g.is_alive())),
            f: Rc::new(move |a| match guard.upgrade_guard() {
                Some(s) => {
                    f(s, a);
                    true
                }
                None => false,
            }),
        });
    }

    /// Runs every live listener through `call` (which invokes it with the
    /// argument) and drops the ones that are dead or ask to disconnect.
    fn notify(&self, call: impl Fn(&dyn Fn(&A) -> bool) -> bool) {
        // Clone first so a listener may connect further listeners.
        let listeners = self.list.borrow().clone();
        let dead: Vec<Listener<A>> = listeners
            .into_iter()
            .filter(|l| !l.is_alive() || !call(&*l.f))
            .collect();
        if !dead.is_empty() {
            self.list
                .borrow_mut()
                .retain(|l| !dead.iter().any(|d| Rc::ptr_eq(&d.f, &l.f)));
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.list.borrow().len()
    }
}

/// Observable selection and hover state, shared by every editor so that
/// selecting or hovering a node anywhere highlights it everywhere
/// (REQUIREMENTS §2.1).
///
/// This is the single source of truth for the selection: the scene's own
/// selection (`Scene::selection`) is not used by the app. Removed nodes
/// are pruned and a new/opened document clears it.
#[derive(Default)]
pub struct Selection {
    selected: RefCell<Vec<NodeId>>,
    hovered: Cell<Option<NodeId>>,
    on_selected: Listeners<Selection>,
    on_hovered: Listeners<Selection>,
}

impl Selection {
    /// Selected nodes, in selection order.
    pub fn selected(&self) -> Vec<NodeId> {
        self.selected.borrow().clone()
    }

    /// Whether `id` is selected (no allocation).
    pub fn is_selected(&self, id: NodeId) -> bool {
        self.selected.borrow().contains(&id)
    }

    /// Node under the pointer in any editor.
    pub fn hovered(&self) -> Option<NodeId> {
        self.hovered.get()
    }

    /// Replaces the selection (duplicates are dropped, order kept).
    pub fn set_selected(&self, ids: impl IntoIterator<Item = NodeId>) {
        let mut unique: Vec<NodeId> = Vec::new();
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        if *self.selected.borrow() != unique {
            *self.selected.borrow_mut() = unique;
            self.on_selected.notify(|f| f(self));
        }
    }

    pub fn set_hovered(&self, id: Option<NodeId>) {
        if self.hovered.replace(id) != id {
            self.on_hovered.notify(|f| f(self));
        }
    }

    /// Calls `f` after every selection or hover change, for the
    /// context's whole lifetime: only for listeners living as long as the
    /// window or app (widget-owned listeners use
    /// [`Selection::connect_changed_for`] and friends). `f` must not
    /// capture the owning [`AppCtx`] strongly.
    pub fn connect_changed(&self, f: impl Fn(&Selection) + 'static) {
        let f = Rc::new(f);
        let g = f.clone();
        self.on_selected.connect(move |s| {
            f(s);
            true
        });
        self.on_hovered.connect(move |s| {
            g(s);
            true
        });
    }

    /// Calls `f` with the upgraded `guard` after every selection or hover
    /// change, while `guard` is alive.
    pub fn connect_changed_for<G: WeakGuard + Clone>(
        &self,
        guard: G,
        f: impl Fn(G::Strong, &Selection) + 'static,
    ) {
        let f = Rc::new(f);
        let g = f.clone();
        self.on_selected
            .connect_for(guard.clone(), move |s, sel| f(s, sel));
        self.on_hovered.connect_for(guard, move |s, sel| g(s, sel));
    }

    /// Calls `f` after the selected set (or its order) changed — not on
    /// hover — while `guard` is alive.
    pub fn connect_selection_for<G: WeakGuard>(
        &self,
        guard: G,
        f: impl Fn(G::Strong, &Selection) + 'static,
    ) {
        self.on_selected.connect_for(guard, f);
    }

    /// Calls `f` after the hovered node changed, while `guard` is alive.
    pub fn connect_hover_for<G: WeakGuard>(
        &self,
        guard: G,
        f: impl Fn(G::Strong, &Selection) + 'static,
    ) {
        self.on_hovered.connect_for(guard, f);
    }

    #[cfg(test)]
    fn listener_count(&self) -> usize {
        self.on_selected.len() + self.on_hovered.len()
    }
}

/// Observable user preferences plus the themes available to choose from.
///
/// Every change goes through [`PrefsStore::update`], which validates the
/// result and notifies listeners when something actually changed.
#[derive(Default)]
pub struct PrefsStore {
    prefs: RefCell<Preferences>,
    listeners: Listeners<Preferences>,
    user_themes: RefCell<Vec<Theme>>,
    /// Desktop color scheme: `Some(true)` for dark, `None` when unknown.
    system_dark: Cell<Option<bool>>,
    /// One-line message about the last load (e.g. settings were reset and
    /// the original backed up), shown in the status bar.
    load_notice: RefCell<Option<String>>,
}

impl PrefsStore {
    /// A copy of the current preferences.
    pub fn get(&self) -> Preferences {
        self.prefs.borrow().clone()
    }

    /// Reads the current preferences without copying them.
    pub fn with<R>(&self, f: impl FnOnce(&Preferences) -> R) -> R {
        f(&self.prefs.borrow())
    }

    /// Edits the preferences; listeners run if anything changed.
    ///
    /// Turning "follow system" on, or changing the theme paired with the
    /// current desktop scheme while following, applies that theme now.
    pub fn update(&self, f: impl FnOnce(&mut Preferences)) {
        let changed = {
            let mut prefs = self.prefs.borrow_mut();
            let before = prefs.clone();
            f(&mut prefs);
            prefs.validate();
            let sys = &prefs.system;
            let follow_on = sys.follow_system && !before.system.follow_system;
            let pair_changed = sys.follow_system
                && (sys.light_theme != before.system.light_theme
                    || sys.dark_theme != before.system.dark_theme);
            if follow_on || pair_changed {
                self.apply_system_theme(&mut prefs);
            }
            *prefs != before
        };
        if changed {
            self.notify();
        }
    }

    /// A manual theme change: edits the preferences and turns "follow
    /// system" off, so the desktop scheme never overrides the choice.
    pub fn update_theme(&self, f: impl FnOnce(&mut Preferences)) {
        self.update(|p| {
            f(p);
            p.system.follow_system = false;
        });
    }

    /// Replaces every preference (revert, factory settings).
    pub fn replace(&self, prefs: Preferences) {
        self.update(move |p| *p = prefs);
    }

    /// Calls `f` after every change. `f` returns `false` to disconnect
    /// (prefer [`PrefsStore::connect_changed_for`] for widget-owned
    /// listeners: dead ones are then pruned without waiting for a change);
    /// it must not capture the owning [`AppCtx`] strongly.
    pub fn connect_changed(&self, f: impl Fn(&Preferences) -> bool + 'static) {
        self.listeners.connect(f);
    }

    /// Calls `f` with the upgraded `guard` after every change, while
    /// `guard` is alive (dropped on the next change or registration once
    /// it is dead).
    pub fn connect_changed_for<G: WeakGuard>(
        &self,
        guard: G,
        f: impl Fn(G::Strong, &Preferences) + 'static,
    ) {
        self.listeners.connect_for(guard, f);
    }

    fn notify(&self) {
        // Each listener gets a fresh snapshot: an earlier listener may have
        // changed the preferences again (which notifies on its own).
        self.listeners.notify(|f| f(&self.get()));
    }

    /// Built-in themes followed by the user's themes.
    pub fn themes(&self) -> Vec<Theme> {
        let mut list = model::builtins();
        list.extend(self.user_themes.borrow().iter().cloned());
        list
    }

    pub fn set_user_themes(&self, themes: Vec<Theme>) {
        *self.user_themes.borrow_mut() = themes;
    }

    pub fn system_dark(&self) -> Option<bool> {
        self.system_dark.get()
    }

    /// Records the desktop color scheme. On a transition (including the
    /// first report at startup), when following the system, switches to
    /// the configured light or dark theme.
    pub fn set_system_dark(&self, dark: Option<bool>) {
        if self.system_dark.replace(dark) != dark {
            self.update(|p| {
                if p.system.follow_system {
                    self.apply_system_theme(p);
                }
            });
        }
    }

    /// Switches to the theme paired with the desktop scheme, if known and
    /// installed. Leaves the theme alone otherwise.
    fn apply_system_theme(&self, prefs: &mut Preferences) {
        let Some(dark) = self.system_dark.get() else {
            return;
        };
        let name = if dark {
            &prefs.system.dark_theme
        } else {
            &prefs.system.light_theme
        };
        if prefs.themes.preset == *name && prefs.theme.name == *name {
            return; // already showing it (possibly with user edits)
        }
        if let Some(theme) = self.themes().into_iter().find(|t| t.name == *name) {
            prefs.themes.preset = theme.name.clone();
            prefs.theme = theme;
        }
    }

    pub fn load_notice(&self) -> Option<String> {
        self.load_notice.borrow().clone()
    }

    pub fn set_load_notice(&self, notice: Option<String>) {
        *self.load_notice.borrow_mut() = notice;
    }
}

struct Inner {
    preferences: PrefsStore,
    selection: Selection,
    engine: EngineHandle,
    providers: RefCell<Vec<Rc<dyn CommandProvider>>>,
}

/// Shared application context. Cloning yields a handle to the same state.
#[derive(Clone)]
pub struct AppCtx {
    inner: Rc<Inner>,
}

impl Default for AppCtx {
    fn default() -> Self {
        Self::with_engine(EngineHandle::default())
    }
}

/// Weak handle to an [`AppCtx`], for closures owned by the context itself.
#[derive(Clone)]
pub struct WeakCtx(std::rc::Weak<Inner>);

impl WeakCtx {
    pub fn upgrade(&self) -> Option<AppCtx> {
        self.0.upgrade().map(|inner| AppCtx { inner })
    }
}

impl AppCtx {
    pub fn new() -> Self {
        Self::default()
    }

    /// A context around `engine`. Scene deletions prune the selection and
    /// clear a hover on a removed node.
    pub fn with_engine(engine: EngineHandle) -> Self {
        let ctx = Self {
            inner: Rc::new(Inner {
                preferences: PrefsStore::default(),
                selection: Selection::default(),
                engine,
                providers: RefCell::default(),
            }),
        };
        let weak = ctx.downgrade();
        ctx.engine().set_pre_broadcast(move |events| {
            let Some(ctx) = weak.upgrade() else {
                return;
            };
            let sel = ctx.selection();
            if let Some(kept) = crate::doc::prune_removed(&sel.selected(), events) {
                sel.set_selected(kept);
            }
            if let Some(h) = sel.hovered()
                && crate::doc::prune_removed(&[h], events).is_some()
            {
                sel.set_hovered(None);
            }
        });
        // A different document: nothing stays selected or hovered.
        let weak = ctx.downgrade();
        ctx.engine()
            .subscribe(move |ev| {
                if let crate::doc::DocEvent::Replaced = ev
                    && let Some(ctx) = weak.upgrade()
                {
                    ctx.selection().set_selected([]);
                    ctx.selection().set_hovered(None);
                }
            })
            .forever();
        ctx
    }

    /// The document engine (scene, evaluation, plotting) and event hub.
    pub fn engine(&self) -> &EngineHandle {
        &self.inner.engine
    }

    pub fn downgrade(&self) -> WeakCtx {
        WeakCtx(Rc::downgrade(&self.inner))
    }

    pub fn preferences(&self) -> &PrefsStore {
        &self.inner.preferences
    }

    pub fn selection(&self) -> &Selection {
        &self.inner.selection
    }

    /// Registers a source of palette commands. Providers are queried every
    /// time the palette opens, so their commands may change over time.
    /// Providers must hold only weak references to widgets and handles that
    /// own this context, to avoid reference cycles.
    pub fn add_command_provider(&self, provider: impl CommandProvider + 'static) {
        self.inner.providers.borrow_mut().push(Rc::new(provider));
    }

    /// Current commands of every provider, in registration order.
    pub fn commands(&self) -> Vec<Command> {
        let providers = self.inner.providers.borrow().clone();
        providers.iter().flat_map(|p| p.commands()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_notifies_on_change_only() {
        let sel = Selection::default();
        let count = Rc::new(Cell::new(0));
        let c = count.clone();
        sel.connect_changed(move |_| c.set(c.get() + 1));

        sel.set_selected([NodeId(3), NodeId(1), NodeId(3)]);
        assert_eq!(sel.selected(), vec![NodeId(3), NodeId(1)]);
        assert_eq!(count.get(), 1);
        sel.set_selected([NodeId(3), NodeId(1)]);
        assert_eq!(count.get(), 1);

        sel.set_hovered(Some(NodeId(7)));
        sel.set_hovered(Some(NodeId(7)));
        assert_eq!(sel.hovered(), Some(NodeId(7)));
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn hover_and_selection_notify_separately_and_dead_listeners_drop() {
        let sel = Selection::default();
        let (s_count, h_count) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let owner = Rc::new(());
        let c = s_count.clone();
        sel.connect_selection_for(Rc::downgrade(&owner), move |_, _| c.set(c.get() + 1));
        let c = h_count.clone();
        sel.connect_hover_for(Rc::downgrade(&owner), move |_, _| c.set(c.get() + 1));
        let gone = Rc::new(());
        let c = s_count.clone();
        sel.connect_selection_for(Rc::downgrade(&gone), move |_, _| c.set(c.get() + 100));
        drop(gone);
        sel.set_hovered(Some(NodeId(1)));
        assert_eq!((s_count.get(), h_count.get()), (0, 1));
        sel.set_selected([NodeId(2)]);
        assert_eq!(
            (s_count.get(), h_count.get()),
            (1, 1),
            "dead listener not called"
        );
        assert_eq!(sel.listener_count(), 2, "the dead listener was dropped");
    }

    #[test]
    fn dead_guarded_listeners_are_pruned_on_registration() {
        // Area churn: editors come and go without any selection or
        // preference change in between.
        let sel = Selection::default();
        let store = PrefsStore::default();
        for _ in 0..50 {
            let editor = Rc::new(());
            sel.connect_changed_for(Rc::downgrade(&editor), |_, _| {});
            store.connect_changed_for(Rc::downgrade(&editor), |_, _| {});
            drop(editor);
        }
        let live = Rc::new(());
        sel.connect_changed_for(Rc::downgrade(&live), |_, _| {});
        store.connect_changed_for(Rc::downgrade(&live), |_, _| {});
        assert_eq!(sel.listener_count(), 2, "one per topic");
        assert_eq!(store.listeners.len(), 1);
        drop(live);
        sel.set_hovered(Some(NodeId(1)));
        sel.set_selected([NodeId(1)]);
        store.update(|p| p.interface.playful = !p.interface.playful);
        assert_eq!(sel.listener_count(), 0);
        assert_eq!(store.listeners.len(), 0);
    }

    #[test]
    fn guarded_listener_gets_the_upgraded_handle() {
        let store = PrefsStore::default();
        let owner = Rc::new(Cell::new(0));
        store.connect_changed_for(Rc::downgrade(&owner), |o, _| o.set(o.get() + 1));
        store.update(|p| p.interface.playful = !p.interface.playful);
        assert_eq!(owner.get(), 1);
    }

    #[test]
    fn listeners_may_connect_listeners() {
        let sel = Rc::new(Selection::default());
        sel.connect_changed(|s| s.connect_changed(|_| {}));
        sel.set_hovered(Some(NodeId(1)));
        sel.set_hovered(None);
    }

    #[test]
    fn prefs_notify_on_change_and_drop_dead_listeners() {
        let store = PrefsStore::default();
        let count = Rc::new(Cell::new(0));
        let c = count.clone();
        store.connect_changed(move |_| {
            c.set(c.get() + 1);
            true
        });
        store.connect_changed(|_| false);
        store.update(|p| p.interface.playful = false);
        store.update(|p| p.interface.playful = false);
        assert_eq!(count.get(), 1);
        assert_eq!(store.listeners.len(), 1);
        // Validation runs on every update.
        store.update(|p| p.interface.ui_scale = 50.0);
        assert_eq!(store.get().interface.ui_scale, 2.0);
    }

    #[test]
    fn prefs_follow_system_scheme() {
        let store = PrefsStore::default();
        store.set_system_dark(Some(false));
        assert!(store.get().theme.dark, "not following yet");
        store.update(|p| p.system.follow_system = true);
        assert_eq!(store.get().theme.name, model::LIGHT_NAME);
        store.set_system_dark(Some(true));
        assert_eq!(store.get().themes.preset, model::DARK_NAME);
    }

    #[test]
    fn follow_system_only_acts_on_transitions() {
        let store = PrefsStore::default();
        store.update(|p| p.system.follow_system = true);
        store.set_system_dark(Some(true));
        // Unrelated updates never swap the theme back.
        store.update(|p| p.theme.ui.bg1 = model::Color::hex(0x123456));
        store.update(|p| p.interface.playful = false);
        assert_eq!(store.get().theme.ui.bg1, model::Color::hex(0x123456));
        // Same scheme reported again: no change.
        store.set_system_dark(Some(true));
        assert_eq!(store.get().theme.ui.bg1, model::Color::hex(0x123456));
    }

    #[test]
    fn manual_theme_choice_turns_following_off() {
        let store = PrefsStore::default();
        store.update(|p| p.system.follow_system = true);
        store.set_system_dark(Some(true));
        store.update_theme(|p| p.theme = model::viscas_light());
        assert!(!store.get().system.follow_system);
        store.set_system_dark(Some(false));
        store.set_system_dark(Some(true));
        assert_eq!(store.get().theme, model::viscas_light());
    }

    #[test]
    fn follow_system_uses_configured_pair() {
        let store = PrefsStore::default();
        let mut custom = model::viscas_light();
        custom.name = "Paper".into();
        store.set_user_themes(vec![custom.clone()]);
        store.update(|p| {
            p.system.follow_system = true;
            p.system.light_theme = "Paper".into();
            p.system.dark_theme = model::HIGH_CONTRAST_NAME.into();
        });
        store.set_system_dark(Some(false));
        assert_eq!(store.get().theme, custom);
        store.set_system_dark(Some(true));
        assert_eq!(store.get().theme.name, model::HIGH_CONTRAST_NAME);
        // A missing pair theme leaves the current theme alone.
        store.update(|p| p.system.light_theme = "Gone".into());
        store.set_system_dark(Some(false));
        assert_eq!(store.get().theme.name, model::HIGH_CONTRAST_NAME);
    }

    #[test]
    fn listeners_see_fresh_preferences() {
        let store = Rc::new(PrefsStore::default());
        let weak = Rc::downgrade(&store);
        store.connect_changed(move |p| {
            if p.interface.playful
                && let Some(s) = weak.upgrade()
            {
                s.update(|q| q.interface.playful = false);
            }
            true
        });
        let seen = Rc::new(Cell::new(true));
        let s = seen.clone();
        store.connect_changed(move |p| {
            s.set(p.interface.playful);
            true
        });
        store.update(|p| p.interface.tooltips = false);
        assert!(!seen.get(), "second listener saw a stale snapshot");
    }

    struct Fixed(&'static str);

    impl CommandProvider for Fixed {
        fn commands(&self) -> Vec<Command> {
            vec![Command {
                id: self.0.to_owned(),
                title: self.0.to_owned(),
                category: "Test",
                shortcut: None,
                run: Rc::new(|| {}),
            }]
        }
    }

    #[test]
    fn commands_come_from_all_providers_in_order() {
        let ctx = AppCtx::new();
        assert!(ctx.commands().is_empty());
        ctx.add_command_provider(Fixed("a"));
        ctx.add_command_provider(Fixed("b"));
        let ids: Vec<_> = ctx.commands().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }
}
