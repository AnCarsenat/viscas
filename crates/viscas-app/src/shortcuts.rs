//! The single table of keyboard accelerators, and the code applying it.

use gtk::prelude::*;

use crate::workspace::Workspace;

/// One accelerator binding.
pub struct Shortcut {
    /// Detailed action name, e.g. `win.workspace::graph`.
    pub action: String,
    /// Accelerator in `gtk::accelerator_parse` syntax.
    pub accel: &'static str,
    /// Description shown in the shortcuts overview.
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
        ("win.undo", "<Control>z", "Undo"),
        ("win.redo", "<Control><Shift>z", "Redo"),
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

/// Registers every accelerator of [`all`] on the application.
pub fn install(app: &gtk::Application) {
    for shortcut in all() {
        app.set_accels_for_action(&shortcut.action, &[shortcut.accel]);
    }
}

/// Human-readable label of an accelerator, e.g. `Ctrl+P`.
fn label(accel: &str) -> String {
    gtk::accelerator_parse(accel)
        .map(|(key, mods)| gtk::accelerator_get_label(key, mods).to_string())
        .unwrap_or_else(|| accel.to_owned())
}

/// Accelerator label of the detailed action `action`, if it has one.
pub fn label_for(action: &str) -> Option<String> {
    all()
        .into_iter()
        .find(|s| s.action == action)
        .map(|s| label(s.accel))
}

/// Key that also opens the palette when focus is not in a text field
/// (Blender's search key). Handled by the window, not an accelerator.
pub const PALETTE_KEY_LABEL: &str = "Space";

/// Hint for the palette button: accelerator and the Space key.
pub fn palette_hint() -> String {
    match label_for("win.palette") {
        Some(accel) => format!("{accel} / {PALETTE_KEY_LABEL}"),
        None => PALETTE_KEY_LABEL.to_owned(),
    }
}

/// Keyboard map for display: key label and description.
pub fn keymap() -> Vec<(String, String)> {
    all()
        .iter()
        .map(|s| {
            let keys = if s.action == "win.palette" {
                palette_hint()
            } else {
                label(s.accel)
            };
            (keys, s.title.clone())
        })
        .collect()
}

/// Plain-text overview of the shortcuts, one per line.
pub fn overview() -> String {
    let mut lines: Vec<String> = keymap()
        .iter()
        .map(|(keys, title)| format!("{keys:<18}{title}"))
        .collect();
    lines.push(format!("{:<18}Zoom graph at cursor", "Scroll"));
    lines.push(format!("{:<18}Pan graph", "Drag"));
    lines.join("\n")
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
