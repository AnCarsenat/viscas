//! Scripts panel: code editor for script nodes.

use super::{Panel, PanelCtx, placeholder};

/// Builds the Scripts panel widget.
pub fn build(_ctx: &PanelCtx) -> gtk::Widget {
    placeholder(Panel::Scripts, "Write and run Viscas scripts.")
}
