//! Blender-style docking (REQUIREMENTS §2, "True docking"): corner drags
//! split and join areas, header drags dock areas onto drop zones or tear
//! them off into floating windows.
//!
//! Every edit is a pure operation on [`model::WorkspaceLayout`]; widgets
//! are rebuilt from the model (see `areas.rs`).
//!
//! **Layout undo.** Layout edits (split, close, join, dock, tear off,
//! editor change, reset, divider drags) are recorded in their own
//! [`model::LayoutHistory`], separate from the document's undo: Ctrl+Z
//! always undoes document (or text) edits, never layout. Layout changes
//! are undone with View ▸ Undo Layout Change (Ctrl+Alt+Z), the palette,
//! or the Undo button of the toast shown after destructive changes
//! ("Area closed · Undo"). The history is cleared when a document is
//! opened or created. Maximize (Ctrl+Space) is transient view state and
//! deliberately not recorded. Undo restores layouts but keeps the visible
//! workspace; editors of closed or moved-away tabs stay parked while a
//! history state still references them, so undo brings them back with
//! their state (viewport view, console input, ...).
//!
//! **Tabs.** An area holds an ordered list of editor tabs (a strip is
//! shown with two or more). Dropping on an area's center docks as a tab
//! (Shift: swap areas, Ctrl: replace); tabs can be dragged to reorder,
//! onto other strips or areas, or out of the window.

pub mod model;
pub mod overlay;
pub mod widgets;
