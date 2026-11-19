//! The single table of keyboard accelerators, and the code applying it.

use adw::prelude::*;

use crate::panels::Panel;

/// One accelerator binding.
pub struct Shortcut {
    /// Detailed action name, e.g. `win.panel::console`.
    pub action: String,
    /// Accelerator in `gtk::accelerator_parse` syntax.
    pub accel: &'static str,
    /// Description shown in the shortcuts overview.
    pub title: String,
}

/// Every accelerator in the app, in display order.
pub fn all() -> Vec<Shortcut> {
    const PANEL_ACCELS: [&str; 5] = [
        "<Control>1",
        "<Control>2",
        "<Control>3",
        "<Control>4",
        "<Control>5",
    ];
    let fixed = [
        ("app.quit", "<Control>q", "Quit"),
        ("app.shortcuts", "<Control>question", "Keyboard shortcuts"),
        ("win.toggle-sidebar", "F9", "Toggle sidebar"),
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
        Panel::ALL
            .iter()
            .zip(PANEL_ACCELS)
            .map(|(panel, accel)| Shortcut {
                action: format!("win.panel::{}", panel.id()),
                accel,
                title: format!("Show {} panel", panel.title()),
            }),
    );
    list
}

/// Registers every accelerator of [`all`] on the application.
pub fn install(app: &adw::Application) {
    for shortcut in all() {
        app.set_accels_for_action(&shortcut.action, &[shortcut.accel]);
    }
}

/// Plain-text overview of the shortcuts, one per line.
pub fn overview() -> String {
    let mut lines: Vec<String> = all()
        .iter()
        .map(|s| {
            let label = gtk::accelerator_parse(s.accel)
                .map(|(key, mods)| gtk::accelerator_get_label(key, mods).to_string())
                .unwrap_or_else(|| s.accel.to_owned());
            format!("{label}\t{}", s.title)
        })
        .collect();
    lines.push("Scroll\tZoom graph at cursor".to_owned());
    lines.push("Drag\tPan graph".to_owned());
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
