//! Application context shared by the window, areas, editors and commands.
//!
//! [`AppCtx`] is created once in `app.rs` and passed down; cloning is cheap.
//! It holds application state that is *not* layout: preferences, the shared
//! selection/hover model, and the command providers the palette queries.
//! The area layout itself stays pure UI state owned by `Areas`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::commands::{Command, CommandProvider};

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

/// User preferences. Placeholder for `feat/preferences`, which will load
/// and save `~/.config/viscas/preferences.toml` and drive the theme.
pub struct Preferences {
    theme: RefCell<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: RefCell::new("Viscas Dark".to_owned()),
        }
    }
}

impl Preferences {
    #[allow(
        dead_code,
        reason = "read by the preferences window (feat/preferences)"
    )]
    pub fn theme_name(&self) -> String {
        self.theme.borrow().clone()
    }
}

#[derive(Default)]
struct Inner {
    preferences: Preferences,
    selection: Selection,
    providers: RefCell<Vec<Rc<dyn CommandProvider>>>,
}

/// Shared application context. Cloning yields a handle to the same state.
#[derive(Clone, Default)]
pub struct AppCtx {
    inner: Rc<Inner>,
}

impl AppCtx {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(
        dead_code,
        reason = "read by the preferences window (feat/preferences)"
    )]
    pub fn preferences(&self) -> &Preferences {
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
