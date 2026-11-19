//! Expressions panel: ordered list of statement nodes, Desmos-style.

use super::{Panel, placeholder};

/// Builds the Expressions panel widget.
pub fn build() -> gtk::Widget {
    placeholder(
        Panel::Expressions,
        "Define functions, plots, points and sliders here.",
    )
}
