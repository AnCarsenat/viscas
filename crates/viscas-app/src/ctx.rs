//! Application context shared by the window, areas, editors and commands.
//!
//! [`AppCtx`] is created once in `app.rs` and passed down; cloning is cheap.
//! It holds application state that is *not* layout: observable preferences
//! (see [`PrefsStore`]), the document engine and its event hub (see
//! [`crate::doc`]), the shared selection/hover model, and the command
//! providers the palette queries.
//! The area layout itself stays pure UI state owned by `Areas`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use viscas_scene::NodeId;

use crate::commands::{Command, CommandProvider};
use crate::doc::EngineHandle;
use crate::prefs::Preferences;
use crate::theme::model::{self, Theme};

type SelectionListener = Rc<dyn Fn(&Selection)>;

/// Observable selection and hover state, shared by every editor so that
/// selecting or hovering a node anywhere highlights it everywhere
/// (REQUIREMENTS §2.1).
#[derive(Default)]
pub struct Selection {
    selected: RefCell<Vec<NodeId>>,
    hovered: Cell<Option<NodeId>>,
    listeners: RefCell<Vec<SelectionListener>>,
}

impl Selection {
    /// Selected nodes, in selection order.
    pub fn selected(&self) -> Vec<NodeId> {
        self.selected.borrow().clone()
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
            self.notify();
        }
    }

    #[allow(dead_code, reason = "driven by the viewport and scene tree")]
    pub fn set_hovered(&self, id: Option<NodeId>) {
        if self.hovered.replace(id) != id {
            self.notify();
        }
    }

    /// Calls `f` after every change. Listeners get the selection as an
    /// argument and must not capture the owning [`AppCtx`] strongly.
    pub fn connect_changed(&self, f: impl Fn(&Selection) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self) {
        // Clone first so a listener may connect further listeners.
        let listeners = self.listeners.borrow().clone();
        for f in listeners {
            f(self);
        }
    }
}

type PrefsListener = Rc<dyn Fn(&Preferences) -> bool>;

/// Observable user preferences plus the themes available to choose from.
///
/// Every change goes through [`PrefsStore::update`], which validates the
/// result and notifies listeners when something actually changed.
#[derive(Default)]
pub struct PrefsStore {
    prefs: RefCell<Preferences>,
    listeners: RefCell<Vec<PrefsListener>>,
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
    /// (e.g. when the widget it updates is gone); it must not capture the
    /// owning [`AppCtx`] strongly.
    pub fn connect_changed(&self, f: impl Fn(&Preferences) -> bool + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self) {
        let listeners = self.listeners.borrow().clone();
        // Each listener gets a fresh snapshot: an earlier listener may have
        // changed the preferences again (which notifies on its own).
        let dead: Vec<PrefsListener> = listeners.into_iter().filter(|f| !f(&self.get())).collect();
        if !dead.is_empty() {
            self.listeners
                .borrow_mut()
                .retain(|f| !dead.iter().any(|d| Rc::ptr_eq(d, f)));
        }
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
        assert_eq!(store.listeners.borrow().len(), 1);
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
