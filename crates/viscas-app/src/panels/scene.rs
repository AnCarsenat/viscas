//! Scene panel: tree view of every node in the document.

use super::{Panel, PanelCtx, placeholder};

/// Builds the Scene panel widget.
pub fn build(_ctx: &PanelCtx) -> gtk::Widget {
    placeholder(Panel::Scene, "Browse, organize and inspect all nodes.")
}
