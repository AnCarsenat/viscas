//! Console panel: `qalc`-style REPL with history and typeset output.

use super::{Panel, PanelCtx, placeholder};

/// Builds the Console panel widget.
pub fn build(_ctx: &PanelCtx) -> gtk::Widget {
    placeholder(
        Panel::Console,
        "Evaluate expressions, convert units and pin results.",
    )
}
