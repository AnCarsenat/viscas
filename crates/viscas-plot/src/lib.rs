//! Plot sampling and view transforms for Viscas.
//!
//! GTK-free plotting engine. Inputs are plain Rust closures, outputs are draw
//! primitives in world coordinates plus a [`View`] transform the UI applies.
//!
//! All samplers are pure functions of their inputs (no global state), accept
//! `?Sized` closure references (so `&dyn Fn(..) + Send + Sync` works) and poll
//! a [`CancelToken`], which makes them suitable for worker threads.

pub mod cancel;
pub mod curve;
pub mod geom;
pub mod options;
pub mod view;

pub use cancel::{CancelToken, Cancelled};
pub use curve::{
    plot_explicit, plot_explicit_on, plot_explicit_x, plot_parametric, plot_polar, sample_explicit,
    sample_explicit_x, sample_parametric,
};
pub use geom::{LineStyle, Polygon, Polyline, Primitive, Rect, Vec2};
pub use options::PlotOptions;
pub use view::{Affine, Ticks, View, nice_step};
