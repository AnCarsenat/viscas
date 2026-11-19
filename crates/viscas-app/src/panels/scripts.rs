//! Scripts panel: code editor for script nodes.

use super::{Panel, placeholder};

/// Builds the Scripts panel widget.
pub fn build() -> gtk::Widget {
    placeholder(Panel::Scripts, "Write and run Viscas scripts.")
}
