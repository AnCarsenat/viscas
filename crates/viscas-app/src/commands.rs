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

/// A source of palette commands, queried each time the palette opens.
pub trait CommandProvider {
    fn commands(&self) -> Vec<Command>;
}

/// The window's static commands (app and window actions).
pub struct WindowCommands {
    window: glib::WeakRef<gtk::ApplicationWindow>,
}

impl WindowCommands {
    pub fn new(window: &gtk::ApplicationWindow) -> Self {
        Self {
            window: window.downgrade(),
        }
    }
}

impl CommandProvider for WindowCommands {
    fn commands(&self) -> Vec<Command> {
        self.window
            .upgrade()
            .map(|w| registry(&w))
            .unwrap_or_default()
    }
}

/// Static description of a command: detailed action name, title, category.
pub fn specs() -> Vec<(String, String, &'static str)> {
    let mut list: Vec<(String, String, &'static str)> = [
        ("win.palette", "Command Palette", "General"),
        ("win.new", "New Document", "File"),
        ("win.open", "Open Document…", "File"),
        ("win.save", "Save Document", "File"),
        ("win.save-as", "Save Document As…", "File"),
        ("win.undo", "Undo", "Edit"),
        ("win.redo", "Redo", "Edit"),
        ("win.toggle-approx", "Toggle Exact / Approx", "Mode"),
        ("win.toggle-angle", "Toggle Radians / Degrees", "Mode"),
        ("app.preferences", "Preferences…", "General"),
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

/// Builds the commands for `window`: each command activates its action on
/// the window (which also resolves `app.` actions).
fn registry(window: &gtk::ApplicationWindow) -> Vec<Command> {
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
            assert!(ids.contains(&s.action), "{} has no command", s.action);
        }
    }
}
