//! Application context shared by the window, areas, editors and commands.
//!
//! [`AppCtx`] is created once in `app.rs` and passed down; cloning is cheap.
//! It holds application state that is *not* layout: observable preferences
//! (see [`PrefsStore`]), the shared
//! selection/hover model, and the command providers the palette queries.
//! The area layout itself stays pure UI state owned by `Areas`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::commands::{Command, CommandProvider};
use crate::prefs::Preferences;
use crate::theme::model::{self, Theme};

/// Placeholder node id until `viscas-scene` provides real node handles.
pub type NodeId = u64;

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
    #[allow(dead_code, reason = "driven by the scene tree and viewport from M2")]
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

    #[allow(dead_code, reason = "driven by the scene tree and viewport from M2")]
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
    pub fn update(&self, f: impl FnOnce(&mut Preferences)) {
        let changed = {
            let mut prefs = self.prefs.borrow_mut();
            let before = prefs.clone();
            f(&mut prefs);
            prefs.validate();
            self.follow_system(&mut prefs);
            *prefs != before
        };
        if changed {
            self.notify();
        }
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
        let prefs = self.get();
        let listeners = self.listeners.borrow().clone();
        let dead: Vec<PrefsListener> = listeners.into_iter().filter(|f| !f(&prefs)).collect();
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

    /// Records the desktop color scheme and, when following it, switches
    /// between the dark and light built-in themes.
    pub fn set_system_dark(&self, dark: Option<bool>) {
        if self.system_dark.replace(dark) != dark {
            self.update(|_| {});
        }
    }

    fn follow_system(&self, prefs: &mut Preferences) {
        let Some(dark) = self.system_dark.get() else {
            return;
        };
        if prefs.system.follow_system && prefs.theme.dark != dark {
            let name = if dark {
                model::DARK_NAME
            } else {
                model::LIGHT_NAME
            };
            if let Some(theme) = model::builtin(name) {
                prefs.theme = theme;
                prefs.themes.preset = name.to_owned();
            }
        }
    }
}

#[derive(Default)]
struct Inner {
    preferences: PrefsStore,
    selection: Selection,
    providers: RefCell<Vec<Rc<dyn CommandProvider>>>,
}

/// Shared application context. Cloning yields a handle to the same state.
#[derive(Clone, Default)]
pub struct AppCtx {
    inner: Rc<Inner>,
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

        sel.set_selected([3, 1, 3]);
        assert_eq!(sel.selected(), vec![3, 1]);
        assert_eq!(count.get(), 1);
        sel.set_selected([3, 1]);
        assert_eq!(count.get(), 1);

        sel.set_hovered(Some(7));
        sel.set_hovered(Some(7));
        assert_eq!(sel.hovered(), Some(7));
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn listeners_may_connect_listeners() {
        let sel = Rc::new(Selection::default());
        sel.connect_changed(|s| s.connect_changed(|_| {}));
        sel.set_hovered(Some(1));
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
