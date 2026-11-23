//! Reusable compact property widgets (preferences now, the Inspector
//! later): Blender-style numeric field, color swatch, labelled property
//! rows and collapsible panels.

pub mod panel;
pub mod row;
pub mod scrub;
pub mod swatch;

pub use panel::Panel;
pub use row::{check_row, property_row};
pub use scrub::ScrubField;
pub use swatch::ColorSwatch;
