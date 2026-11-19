//! Sampling options shared by all plot kinds.

use crate::cancel::CancelToken;

/// Tuning knobs for the samplers. The defaults target a crisp result at
/// 1:1 pixel density while staying well within a 60 fps frame budget.
#[derive(Clone, Debug)]
pub struct PlotOptions {
    /// Max distance (px) between the polyline and the sampled midpoint.
    pub tolerance_px: f64,
    /// Max polyline segment length (px). Segments still longer than this at
    /// `max_depth` are tested for a discontinuity (jump or asymptote).
    pub max_chord_px: f64,
    /// Spacing (px) of the initial uniform samples for explicit curves.
    pub initial_spacing_px: f64,
    /// Number of initial uniform samples for parametric/polar curves.
    pub parametric_samples: usize,
    /// Max recursive subdivision depth below an initial interval.
    pub max_depth: u32,
    /// Max function evaluations per curve before refinement stops.
    pub max_evals: usize,
    /// Margin (fraction of the view size per side) of the clip rectangle.
    pub clip_margin: f64,
    /// Coarse grid cell size (px) for implicit curves and regions.
    pub implicit_cell_px: f64,
    /// Quadtree refinement levels of sign-change cells (finest cell is
    /// `implicit_cell_px / 2^implicit_depth`).
    pub implicit_depth: u32,
    /// Cooperative cancellation flag.
    pub cancel: CancelToken,
}

impl Default for PlotOptions {
    fn default() -> Self {
        Self {
            tolerance_px: 0.5,
            max_chord_px: 2.0,
            initial_spacing_px: 3.0,
            parametric_samples: 256,
            max_depth: 14,
            max_evals: 200_000,
            clip_margin: 0.25,
            implicit_cell_px: 8.0,
            implicit_depth: 3,
            cancel: CancelToken::none(),
        }
    }
}

impl PlotOptions {
    /// Default options with the given cancel token.
    pub fn with_cancel(cancel: CancelToken) -> Self {
        Self {
            cancel,
            ..Self::default()
        }
    }
}
