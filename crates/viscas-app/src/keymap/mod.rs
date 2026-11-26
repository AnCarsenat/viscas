//! Keymap: every bindable action, its default and current key chords,
//! the persisted overrides, and the live dispatch of key presses.

pub mod live;
pub mod model;
pub mod registry;

pub use model::{Context, KeymapPrefs};
