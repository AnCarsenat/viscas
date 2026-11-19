//! Sheets panel: spreadsheet editor for table nodes.

use super::{Panel, PanelCtx, placeholder};

/// Builds the Sheets panel widget.
pub fn build(_ctx: &PanelCtx) -> gtk::Widget {
    placeholder(Panel::Sheets, "Edit tables whose columns can be plotted.")
}
