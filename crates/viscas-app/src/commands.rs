//! Central action registry: every user-facing command with its id, title,
//! shortcut and callback. The command palette lists these; menus and
//! accelerators refer to the same detailed action names.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::layout::{Dir, EditorKind};
use crate::shortcuts;
use crate::workspace::Workspace;

/// One registered command.
pub struct Command {
    /// Detailed action name, e.g. `win.workspace::graph`. Unique.
    pub id: String,
    /// Title shown in the palette.
    pub title: String,
    /// Group label shown next to the title.
    pub category: &'static str,
    /// Human-readable accelerator label, if the command has one.
    pub shortcut: Option<String>,
    /// Runs the command.
    pub run: Rc<dyn Fn()>,
}

/// Static description of a command: detailed action name, title, category.
pub fn specs() -> Vec<(String, String, &'static str)> {
    let mut list: Vec<(String, String, &'static str)> = [
        ("win.palette", "Command Palette", "General"),
        ("app.shortcuts", "Keyboard Shortcuts", "Help"),
        ("app.about", "About Viscas", "Help"),
        ("app.quit", "Quit", "General"),
        ("win.maximize-area", "Maximize / Restore Area", "Area"),
        ("win.close-area", "Close Area (Join)", "Area"),
    ]
    .into_iter()
    .map(|(a, t, c)| (a.to_owned(), t.to_owned(), c))
    .collect();
    for (dir, title) in [
        (Dir::Horizontal, "Split Area Horizontally"),
        (Dir::Vertical, "Split Area Vertically"),
    ] {
        list.push((
            format!("win.split-area::{}", dir.id()),
            title.to_owned(),
            "Area",
        ));
    }
    for ws in Workspace::ALL {
        list.push((
            format!("win.workspace::{}", ws.id()),
            format!("Workspace: {}", ws.title()),
            "Workspace",
        ));
    }
    for kind in EditorKind::ALL {
        list.push((
            format!("win.editor::{}", kind.id()),
            format!("Editor: {}", kind.title()),
            "Editor",
        ));
    }
    list
}

/// Builds the registry for `window`: each command activates its action on
/// the window (which also resolves `app.` actions).
pub fn registry(window: &gtk::ApplicationWindow) -> Vec<Command> {
    specs()
        .into_iter()
        .map(|(id, title, category)| {
            let weak = window.downgrade();
            let detailed = id.clone();
            let run: Rc<dyn Fn()> = Rc::new(move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                match gio::Action::parse_detailed_name(&detailed) {
                    Ok((name, target)) => {
                        if let Err(err) =
                            WidgetExt::activate_action(&window, &name, target.as_ref())
                        {
                            glib::g_warning!("viscas", "command {detailed}: {err}");
                        }
                    }
                    Err(err) => glib::g_warning!("viscas", "bad command id {detailed}: {err}"),
                }
            });
            Command {
                shortcut: shortcuts::label_for(&id),
                id,
                title,
                category,
                run,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::specs;

    #[test]
    fn ids_are_unique_and_parseable() {
        let list = specs();
        for (i, (a, _, _)) in list.iter().enumerate() {
            assert!(a.starts_with("app.") || a.starts_with("win."));
            for (b, _, _) in &list[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn every_shortcut_has_a_command() {
        let ids: Vec<String> = specs().into_iter().map(|(a, _, _)| a).collect();
        for s in crate::shortcuts::all() {
            if s.action.starts_with("win.undo") || s.action.starts_with("win.redo") {
                continue; // placeholders until the scene has an undo stack
            }
            assert!(ids.contains(&s.action), "{} has no command", s.action);
        }
    }
}
