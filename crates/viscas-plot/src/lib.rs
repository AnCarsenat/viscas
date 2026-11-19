//! Plot sampling and view transforms for Viscas.
//!
//! GTK-free plotting engine. Inputs are plain Rust closures, outputs are draw
//! primitives in world coordinates plus a [`View`] transform the UI applies.
//!
//! All samplers are pure functions of their inputs (no global state), accept
//! `?Sized` closure references (so `&dyn Fn(..) + Send + Sync` works) and poll
//! a [`CancelToken`], which makes them suitable for worker threads.
//!
//! | Plot kind | Entry point |
//! |-----------|-------------|
//! | `y = f(x)`, `x = f(y)` | [`plot_explicit`], [`plot_explicit_on`], [`plot_explicit_x`] |
//! | parametric, polar | [`plot_parametric`], [`plot_polar`] |
//! | `F(x, y) = 0` | [`plot_implicit`] |
//! | inequalities | [`plot_explicit_inequality`], [`plot_explicit_x_inequality`], [`plot_implicit_inequality`] |
//! | points | [`plot_points`], [`plot_point_list`] |
//! | roots / extrema / intersections | [`find_roots`], [`find_extrema`], [`find_intersections`], [`explicit_features`] |
//!
//! ```
//! use viscas_plot::{plot_explicit, PlotOptions, View};
//!
//! let view = View::default(); // 1200×800 px, x ∈ [-10, 10]
//! let lines = plot_explicit(&|x: f64| 1.0 / x, &view, &PlotOptions::default()).unwrap();
//! assert_eq!(lines.len(), 2); // no line joins the branches across x = 0
//!
//! // The UI maps world points to pixels with the view transform.
//! let t = view.transform();
//! let px = t.apply(lines[1].points[0]);
//! assert!(px.x > 600.0);
//! ```

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
