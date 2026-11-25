//! Command palette sources beyond actions (REQUIREMENTS §2.1.3): nodes
//! ("Go to node"), functions (insert into the console) and settings (open
//! the preferences page).

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::commands::{Command, CommandProvider};
use crate::ctx::{AppCtx, WeakCtx};
use crate::editors::scene_tree::model;

/// Registers every source for `window`.
pub fn install(window: &gtk::ApplicationWindow, ctx: &AppCtx) {
    ctx.add_command_provider(NodeIndex(ctx.downgrade()));
    ctx.add_command_provider(FunctionIndex);
    ctx.add_command_provider(SettingsIndex {
        window: window.downgrade(),
        ctx: ctx.downgrade(),
    });
    ctx.add_command_provider(crate::viewport::ViewportCommands::new(ctx));
}

/// "Go to node: Plot1": selects the node (the tree reveals it) and frames
/// it in the active viewport.
struct NodeIndex(WeakCtx);

impl CommandProvider for NodeIndex {
    fn commands(&self) -> Vec<Command> {
        let Some(ctx) = self.0.upgrade() else {
            return Vec::new();
        };
        let nodes: Vec<(viscas_scene::NodeId, String)> = ctx.engine().with(|e| {
            let scene = e.scene();
            scene
                .iter()
                .map(|n| (n.id, model::display_name(scene, n.id)))
                .collect()
        });
        nodes
            .into_iter()
            .map(|(id, name)| {
                let weak = self.0.clone();
                Command {
                    id: format!("node.goto::{}", id.0),
                    title: format!("Go to node: {}", model::one_line(&name, 48)),
                    category: "Node",
                    shortcut: None,
                    run: Rc::new(move || {
                        let Some(ctx) = weak.upgrade() else {
                            return;
                        };
                        ctx.selection().set_selected([id]);
                        if let Some(vp) = crate::viewport::active() {
                            vp.frame_node(id);
                        }
                    }),
                }
            })
            .collect()
    }
}

/// Builtin functions: inserts `name(` into the console input.
struct FunctionIndex;

impl CommandProvider for FunctionIndex {
    fn commands(&self) -> Vec<Command> {
        viscas_engine::help::FUNCTION_HELP
            .iter()
            .map(|h| {
                let insert = format!("{}(", h.name);
                Command {
                    id: format!("function.insert::{}", h.name),
                    title: format!("{} — {}", h.signature, h.summary),
                    category: "Function",
                    shortcut: None,
                    run: Rc::new(move || crate::editors::console::insert_into_active(&insert)),
                }
            })
            .collect()
    }
}

/// Settings the palette can jump to: (preferences page, title).
pub const SETTINGS: &[(&str, &str)] = &[
    ("interface", "Appearance: Dark / Light / System"),
    ("interface", "Accent Color"),
    ("interface", "Resolution Scale"),
    ("interface", "Font Size"),
    ("interface", "Monospace Font"),
    ("interface", "Font Rendering"),
    ("interface", "Tooltips"),
    ("interface", "Playful Microcopy"),
    ("interface", "Show Unfinished Editors"),
    ("themes", "Theme Preset"),
    ("themes", "Import / Export Theme"),
    ("themes", "Advanced Theme Editing"),
    ("viewport", "Viewport Grid, Axes and Labels"),
    ("editing", "Definition Operator (:= or =)"),
    ("editing", "Undo Steps"),
    ("editing", "Console Adds Plots to Board"),
    ("keymap", "Keymap"),
    ("save-load", "Save & Load Preferences"),
    ("system", "Follow System Light/Dark"),
];

struct SettingsIndex {
    window: glib::WeakRef<gtk::ApplicationWindow>,
    ctx: WeakCtx,
}

impl CommandProvider for SettingsIndex {
    fn commands(&self) -> Vec<Command> {
        SETTINGS
            .iter()
            .map(|(page, title)| {
                let (window, ctx) = (self.window.clone(), self.ctx.clone());
                let page = *page;
                Command {
                    id: format!("setting::{page}::{title}"),
                    title: format!("Setting: {title}"),
                    category: "Settings",
                    shortcut: None,
                    run: Rc::new(move || {
                        let app = window.upgrade().and_then(|w| w.application());
                        if let (Some(app), Some(ctx)) = (app, ctx.upgrade()) {
                            crate::prefs_window::present(&app, &ctx, Some(page));
                        }
                    }),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_point_at_existing_pages() {
        let pages = [
            "interface",
            "themes",
            "viewport",
            "editing",
            "keymap",
            "save-load",
            "system",
        ];
        for (page, _) in super::SETTINGS {
            assert!(pages.contains(page), "{page}");
        }
    }
}
