//! Plot sampling and view transforms for Viscas.
//!
//! GTK-free plotting engine. Inputs are plain Rust closures, outputs are draw
//! primitives in world coordinates plus a [`View`] transform the UI applies.
//!
//! All samplers are pure functions of their inputs (no global state), accept
//! `?Sized` closure references (so `&dyn Fn(..) + Send + Sync` works) and poll
//! a [`CancelToken`], which makes them suitable for worker threads.

pub mod analysis;
pub mod cancel;
pub mod curve;
pub mod geom;
pub mod implicit;
pub mod inequality;
pub mod options;
pub mod points;
pub mod view;

pub use analysis::{
    Feature, FeatureKind, brent_root, explicit_features, find_extrema, find_intersections,
    find_roots, golden_min, nearest_feature,
};
pub use cancel::{CancelToken, Cancelled};
pub use curve::{
    plot_explicit, plot_explicit_on, plot_explicit_x, plot_parametric, plot_polar, sample_explicit,
    sample_explicit_x, sample_parametric,
};
pub use geom::{LineStyle, Polygon, Polyline, Primitive, Rect, Vec2};
pub use implicit::plot_implicit;
pub use inequality::{
    Comparison, Region, plot_explicit_inequality, plot_explicit_x_inequality,
    plot_implicit_inequality,
};
pub use options::PlotOptions;
pub use points::{plot_point_list, plot_points};
pub use view::{Affine, Ticks, View, nice_step};
