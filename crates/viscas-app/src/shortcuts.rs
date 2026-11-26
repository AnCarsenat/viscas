//! Default keyboard accelerators of the window commands (the Viscas
//! preset). The keymap (`crate::keymap`) reads them as defaults; the
//! user's bindings and all key handling live there.

use crate::workspace::Workspace;

/// One default accelerator.
pub struct Shortcut {
    /// Detailed action name, e.g. `win.workspace::graph`.
    pub action: String,
    /// Accelerator in `gtk::accelerator_parse` syntax.
    pub accel: &'static str,
    /// Description, for actions without a palette command.
    pub title: String,
}

/// Every accelerator in the app, in display order.
pub fn all() -> Vec<Shortcut> {
    const WORKSPACE_ACCELS: [&str; 4] = [
        "<Control><Alt>1",
        "<Control><Alt>2",
        "<Control><Alt>3",
        "<Control><Alt>4",
    ];
    let fixed = [
        ("win.palette", "<Control>p", "Command palette"),
        ("app.preferences", "<Control>comma", "Preferences"),
        (
            "win.maximize-area",
            "<Control>space",
            "Maximize / restore area",
        ),
        ("app.quit", "<Control>q", "Quit"),
        ("app.shortcuts", "<Control>question", "Keyboard shortcuts"),
        ("win.new", "<Control>n", "New document"),
        ("win.open", "<Control>o", "Open document"),
        ("win.save", "<Control>s", "Save document"),
        ("win.save-as", "<Control><Shift>s", "Save document as"),
        ("win.export", "<Control><Shift>e", "Export"),
        ("win.undo", "<Control>z", "Undo"),
        ("win.redo", "<Control><Shift>z", "Redo"),
        ("win.layout-undo", "<Control><Alt>z", "Undo layout change"),
        ("win.next-tab", "<Control>Tab", "Next tab in area"),
        (
            "win.prev-tab",
            "<Control><Shift>Tab",
            "Previous tab in area",
        ),
    ];
    let mut list: Vec<Shortcut> = fixed
        .into_iter()
        .map(|(action, accel, title)| Shortcut {
            action: action.to_owned(),
            accel,
            title: title.to_owned(),
        })
        .collect();
    list.extend(
        Workspace::ALL
            .iter()
            .zip(WORKSPACE_ACCELS)
            .map(|(ws, accel)| Shortcut {
                action: format!("win.workspace::{}", ws.id()),
                accel,
                title: format!("{} workspace", ws.title()),
            }),
    );
    list
}

#[cfg(test)]
mod tests {
    use super::all;

    #[test]
    fn accels_and_actions_are_unique() {
        let list = all();
        for (i, a) in list.iter().enumerate() {
            for b in &list[i + 1..] {
                assert_ne!(a.accel, b.accel);
                assert_ne!(a.action, b.action);
            }
        }
    }
}
