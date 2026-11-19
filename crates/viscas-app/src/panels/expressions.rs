//! Expressions panel: ordered list of statement nodes, Desmos-style.

use super::{Panel, PanelCtx, placeholder};

/// Builds the Expressions panel widget.
pub fn build(_ctx: &PanelCtx) -> gtk::Widget {
    placeholder(
        Panel::Expressions,
        "Define functions, plots, points and sliders here.",
    )
}
