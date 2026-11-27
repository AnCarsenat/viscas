//! The single list of bindable actions: window commands (from
//! [`crate::commands::specs`], defaults from [`crate::shortcuts::all`]),
//! editor-local keys (viewport, scene tree, console) and actions other
//! modules register at startup (text-editing features).

use std::cell::RefCell;

use super::model::{ActionSpec, Context};

thread_local! {
    static EXTRA: RefCell<Vec<ActionSpec>> = const { RefCell::new(Vec::new()) };
}

/// Registers an extra bindable action (replacing one with the same id),
/// e.g. a text-editing feature's key. Call before windows are built; the
/// keymap editor lists it and [`super::live`] resolves its bindings.
pub fn register(spec: ActionSpec) {
    EXTRA.with_borrow_mut(|list| {
        list.retain(|s| s.id != spec.id);
        list.push(spec);
    });
    super::live::specs_changed();
}

/// Registers a "Text Editing" action with default keys such as
/// `"Ctrl+space"`. Handlers check it with [`super::live::matches`].
#[allow(dead_code, reason = "registration hook for text-editing features")]
pub fn register_text_action(id: &str, title: &str, defaults: &[&str]) {
    register(ActionSpec::new(
        id,
        title,
        "Text Editing",
        Context::TextEditing,
        defaults,
    ));
}

/// Keymap-editor category of a command category.
pub fn category(command_category: &str) -> &str {
    match command_category {
        "Area" => "Area & Docking",
        "General" | "Help" => "Window",
        other => other,
    }
}

/// Window actions that should not fire again while their key is held.
const NO_REPEAT: [&str; 4] = [
    "win.palette",
    "win.maximize-area",
    "app.preferences",
    "app.shortcuts",
];

/// Palette command categories whose commands are per-item and not worth
/// binding (one per node, function or setting).
const UNBINDABLE_CATEGORIES: [&str; 3] = ["Node", "Function", "Settings"];

/// The registry plus the bindable commands `commands` (from the palette
/// providers) that it does not know yet, as Window actions without
/// defaults. Commands other modules add show up here automatically.
pub fn with_commands(commands: &[crate::commands::Command]) -> Vec<ActionSpec> {
    let mut out = super::live::specs();
    for cmd in commands {
        if UNBINDABLE_CATEGORIES.contains(&cmd.category) || out.iter().any(|s| s.id == cmd.id) {
            continue;
        }
        out.push(ActionSpec::new(
            &cmd.id,
            &cmd.title,
            category(cmd.category),
            Context::Window,
            &[],
        ));
    }
    out
}

/// Every bindable action, in display order.
pub fn specs() -> Vec<ActionSpec> {
    let shortcuts = crate::shortcuts::all();
    let mut out: Vec<ActionSpec> = Vec::new();
    for (id, title, cat) in crate::commands::specs() {
        let mut keys: Vec<&str> = shortcuts
            .iter()
            .filter(|s| s.action == id)
            .map(|s| s.accel)
            .collect();
        if id == "win.palette" {
            // Blender's search key, outside text fields.
            keys.push("space");
        }
        let mut spec = ActionSpec::new(&id, &title, category(cat), Context::Window, &keys);
        if NO_REPEAT.contains(&id.as_str()) {
            spec = spec.no_repeat();
        }
        if id == "win.palette" {
            spec = spec.blender(&["F3"]);
        }
        out.push(spec);
    }
    // Accelerators of actions without a palette command.
    for s in &shortcuts {
        if !out.iter().any(|o| o.id == s.action) {
            out.push(ActionSpec::new(
                &s.action,
                &s.title,
                "Window",
                Context::Window,
                &[s.accel],
            ));
        }
    }
    out.extend([
        ActionSpec::new(
            "viewport.reset-view",
            "Reset View",
            "Viewport",
            Context::Viewport,
            &["Home"],
        ),
        ActionSpec::new(
            "viewport.clear-marks",
            "Clear Dots and Selection",
            "Viewport",
            Context::Viewport,
            &["Escape"],
        ),
        ActionSpec::new(
            "viewport.export-box",
            "Export Box Region (drag a box)",
            "Viewport",
            Context::Viewport,
            &["Shift+e"],
        ),
        ActionSpec::new(
            "scene.rename",
            "Rename Node",
            "Scene Tree",
            Context::SceneTree,
            &["F2"],
        ),
        ActionSpec::new(
            "scene.delete",
            "Delete Selected Nodes",
            "Scene Tree",
            Context::SceneTree,
            &["Delete"],
        )
        .blender(&["x", "Delete"]),
        ActionSpec::new(
            "scene.duplicate",
            "Duplicate Selected Nodes",
            "Scene Tree",
            Context::SceneTree,
            &["Ctrl+d"],
        )
        .blender(&["Shift+d"]),
        ActionSpec::new(
            "scene.group",
            "Group Selected into Folder",
            "Scene Tree",
            Context::SceneTree,
            &["Ctrl+g"],
        ),
        ActionSpec::new(
            "console.add-to-board",
            "Add Last Result to Board",
            "Console",
            Context::Console,
            &["Ctrl+Shift+Return"],
        ),
        ActionSpec::new(
            "console.clear",
            "Clear Transcript",
            "Console",
            Context::Console,
            &["Ctrl+l"],
        ),
    ]);
    EXTRA.with_borrow(|extra| {
        for spec in extra {
            out.retain(|s| s.id != spec.id);
            out.push(spec.clone());
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::model::{Chord, KeymapPrefs, Preset};

    #[test]
    fn ids_are_unique_and_every_accelerator_is_registered() {
        let list = specs();
        for (i, a) in list.iter().enumerate() {
            assert!(list[i + 1..].iter().all(|b| b.id != a.id), "{}", a.id);
        }
        for s in crate::shortcuts::all() {
            let spec = list.iter().find(|a| a.id == s.action).unwrap();
            assert_eq!(
                spec.defaults[0],
                Chord::parse(s.accel).unwrap(),
                "{}",
                s.action
            );
        }
    }

    #[test]
    fn default_presets_have_no_conflicts() {
        let list = specs();
        for preset in Preset::ALL {
            let mut km = KeymapPrefs::default();
            km.set_preset(preset, &list);
            assert!(
                km.conflicts(&list).is_empty(),
                "{preset:?}: {:?}",
                km.conflicts(&list)
            );
        }
    }

    #[test]
    fn registered_actions_join_the_registry() {
        register_text_action("text.test-complete", "Complete", &["Ctrl+space"]);
        let list = specs();
        let spec = list.iter().find(|s| s.id == "text.test-complete").unwrap();
        assert_eq!(spec.context, Context::TextEditing);
        assert_eq!(spec.defaults, vec![Chord::parse("Ctrl+space").unwrap()]);
        // Ctrl+Space is also Maximize Area: a Window key fires in text
        // fields too, so the two conflict.
        let km = KeymapPrefs::default();
        let found = km.conflicts(&list);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ids.contains(&"win.maximize-area".to_owned()));
    }
}
