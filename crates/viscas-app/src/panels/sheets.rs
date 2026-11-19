//! Sheets panel: spreadsheet editor for table nodes.

use super::{Panel, placeholder};

/// Builds the Sheets panel widget.
pub fn build() -> gtk::Widget {
    placeholder(Panel::Sheets, "Edit tables whose columns can be plotted.")
}
