//! Compiled plots and their rendering into draw primitives.
//!
//! A [`PlotShape`] holds `Send + Sync` closures compiled from a node by the
//! evaluator (scene values are folded in as constants), so it can be
//! rendered on any thread. [`PlotJob`] bundles the shapes of one viewport
//! with a [`View`] and a [`CancelToken`].

use std::f64::consts::TAU;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use viscas_plot::{
    CancelToken, Cancelled, Comparison, PlotOptions, Primitive, Vec2, View, plot_explicit,
    plot_explicit_inequality, plot_explicit_x, plot_explicit_x_inequality, plot_implicit,
    plot_implicit_inequality, plot_parametric, plot_point_list, plot_polar,
};
use viscas_scene::{Color, LineStyle, NodeId, Style};

/// A real function of one variable (NaN = undefined).
pub type Fn1 = Arc<dyn Fn(f64) -> f64 + Send + Sync>;
/// A real function of two variables (NaN = undefined).
pub type Fn2 = Arc<dyn Fn(f64, f64) -> f64 + Send + Sync>;

/// What kind of plot a node is (for the UI: icons, inspector, legends).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlotKind {
    ExplicitY,
    ExplicitX,
    Parametric,
    Polar,
    PolarAngle,
    Implicit,
    Inequality,
    PolarInequality,
    Points,
}

impl PlotKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PlotKind::ExplicitY => "explicit y",
            PlotKind::ExplicitX => "explicit x",
            PlotKind::Parametric => "parametric",
            PlotKind::Polar => "polar",
            PlotKind::PolarAngle => "polar angle",
            PlotKind::Implicit => "implicit",
            PlotKind::Inequality => "inequality",
            PlotKind::PolarInequality => "polar inequality",
            PlotKind::Points => "points",
        }
    }
}

impl fmt::Display for PlotKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compiled geometry of a plot-like node.
#[derive(Clone)]
pub enum PlotShape {
    /// `y = f(x)`.
    ExplicitY(Fn1),
    /// `x = f(y)`.
    ExplicitX(Fn1),
    /// `(x(t), y(t))` for `t` in `range`.
    Parametric { x: Fn1, y: Fn1, range: (f64, f64) },
    /// `r = f(θ)` (θ in radians) for `θ` in `range`.
    Polar { r: Fn1, range: (f64, f64) },
    /// `θ = g(r)` (radians), drawn for `r` across the view.
    PolarAngle { theta: Fn1 },
    /// `F(x, y) = 0`.
    Implicit(Fn2),
    /// `y ⋚ f(x)`.
    InequalityY { f: Fn1, cmp: Comparison },
    /// `x ⋚ f(y)`.
    InequalityX { f: Fn1, cmp: Comparison },
    /// `F(x, y) ⋚ 0` (also polar inequalities converted to x/y and
    /// chained comparisons as a ±1 indicator).
    InequalityXY { f: Fn2, cmp: Comparison },
    /// Points, optionally connected.
    Points { points: Vec<Vec2>, connect: bool },
}

impl fmt::Debug for PlotShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            PlotShape::ExplicitY(_) => "ExplicitY",
            PlotShape::ExplicitX(_) => "ExplicitX",
            PlotShape::Parametric { .. } => "Parametric",
            PlotShape::Polar { .. } => "Polar",
            PlotShape::PolarAngle { .. } => "PolarAngle",
            PlotShape::Implicit(_) => "Implicit",
            PlotShape::InequalityY { .. } => "InequalityY",
            PlotShape::InequalityX { .. } => "InequalityX",
            PlotShape::InequalityXY { .. } => "InequalityXY",
            PlotShape::Points { points, .. } => {
                return write!(f, "Points({})", points.len());
            }
        };
        f.write_str(name)
    }
}

/// Recompiles a shape whose closures fall back to the general evaluator,
/// with the given flag as `EvalOptions::interrupt`, so a superseded job
/// stops even inside one slow sample.
pub type Rebuild = Arc<dyn Fn(Arc<AtomicBool>) -> Option<PlotShape> + Send + Sync>;

/// A node's compiled plot.
#[derive(Clone)]
pub struct CompiledPlot {
    pub kind: PlotKind,
    pub shape: PlotShape,
    /// Present when some closure uses the (slow, interruptible) general
    /// evaluator; jobs rebuild the shape with their own interrupt flag.
    pub rebuild: Option<Rebuild>,
}

impl fmt::Debug for CompiledPlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledPlot")
            .field("kind", &self.kind)
            .field("shape", &self.shape)
            .field("rebuild", &self.rebuild.is_some())
            .finish()
    }
}

impl CompiledPlot {
    pub fn new(kind: PlotKind, shape: PlotShape) -> CompiledPlot {
        CompiledPlot {
            kind,
            shape,
            rebuild: None,
        }
    }

    /// The shape bound to `flag` (itself when no closure needs it).
    pub fn for_job(self: &Arc<Self>, flag: &Arc<AtomicBool>) -> Arc<CompiledPlot> {
        match self.rebuild.as_ref().and_then(|r| r(flag.clone())) {
            Some(shape) => Arc::new(CompiledPlot::new(self.kind, shape)),
            None => self.clone(),
        }
    }
}

impl PlotKind {
    /// Draw rank: fills (0) under curves (1) under points (2).
    pub fn draw_rank(self) -> u8 {
        match self {
            PlotKind::Inequality | PlotKind::PolarInequality => 0,
            PlotKind::Points => 2,
            _ => 1,
        }
    }
}

/// Visual style of a layer, resolved from the node style (automatic
/// palette color filled in).
#[derive(Clone, Debug, PartialEq)]
pub struct PlotStyle {
    pub color: Color,
    pub line_width: f64,
    pub line_style: LineStyle,
    pub opacity: f64,
    pub fill_opacity: f64,
    pub point_size: f64,
    pub label: Option<String>,
    pub show_label: bool,
}

impl PlotStyle {
    pub fn from_style(style: &Style, auto: Color) -> PlotStyle {
        PlotStyle {
            color: style.color.unwrap_or(auto),
            line_width: style.line_width,
            line_style: style.line_style,
            opacity: style.opacity,
            fill_opacity: style.fill_opacity,
            point_size: style.point_size,
            label: style.label.clone(),
            show_label: style.show_label,
        }
    }
}

/// Draw output of one node in one viewport, world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotLayer {
    pub node_id: NodeId,
    pub kind: PlotKind,
    pub style: PlotStyle,
    /// Shared, so handing layers to the UI never deep-copies geometry.
    pub primitives: Arc<Vec<Primitive>>,
    /// The evaluation budget cut sampling short (coarser result).
    pub truncated: bool,
    /// Rendering failed (e.g. a worker panic); `primitives` is empty and
    /// the layer is retried on the next request.
    pub error: Option<String>,
    /// Computed for an older view or revision; still drawable (world
    /// coordinates) until the fresh result arrives. Set by
    /// [`crate::Engine::layers`].
    pub stale: bool,
}

/// One node of a [`PlotJob`].
#[derive(Clone, Debug)]
pub struct PlotItem {
    pub node: NodeId,
    /// Evaluation revision of the node the shape was compiled at.
    pub revision: u64,
    pub style: PlotStyle,
    pub plot: Arc<CompiledPlot>,
}

/// A viewport allocated by [`crate::Engine::create_viewport`] (or mapped
/// from a scene Viewport node by [`crate::Engine::viewport_for_node`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewportId(pub(crate) u64);

impl ViewportId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A snapshot of compiled plots for one viewport and view. `Send + Sync`;
/// render it with [`PlotJob::run`] (any thread) or let the engine's worker
/// pool do it ([`crate::Engine::request_plots`]).
#[derive(Clone, Debug)]
pub struct PlotJob {
    pub viewport: ViewportId,
    /// Engine-wide increasing job number; results of jobs older than the
    /// viewport's last cancellation are discarded.
    pub generation: u64,
    pub view: View,
    pub items: Vec<PlotItem>,
    pub cancel: CancelToken,
    pub options: PlotOptions,
}

impl PlotJob {
    /// Renders every item; `Err` if the job was cancelled.
    pub fn run(&self) -> Result<Vec<PlotLayer>, Cancelled> {
        (0..self.items.len()).map(|i| self.run_item(i)).collect()
    }

    /// Renders item `i`. A panic becomes an error layer. `Err` when the
    /// job is cancelled before, during or right after sampling (samples
    /// interrupted by the job flag are NaN, so such a result is never
    /// returned).
    pub fn run_item(&self, i: usize) -> Result<PlotLayer, Cancelled> {
        let item = &self.items[i];
        self.cancel.check()?;
        let rendered = catch_unwind(AssertUnwindSafe(|| {
            render(&item.plot.shape, &self.view, &self.options)
        }));
        let (primitives, truncated, error) = match rendered {
            Ok(r) => {
                let (p, t) = r?;
                (p, t, None)
            }
            Err(_) => (
                Vec::new(),
                false,
                Some("plotting failed (internal error)".into()),
            ),
        };
        self.cancel.check()?;
        Ok(PlotLayer {
            node_id: item.node,
            kind: item.plot.kind,
            style: item.style.clone(),
            primitives: Arc::new(primitives),
            truncated,
            error,
            stale: false,
        })
    }

    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }
}

/// Largest distance from the origin to a view corner (for rays).
fn view_radius(view: &View) -> f64 {
    let b = view.bounds();
    [b.x_min.abs(), b.x_max.abs()]
        .iter()
        .flat_map(|x| [b.y_min.abs(), b.y_max.abs()].map(|y| x.hypot(y)))
        .fold(0.0, f64::max)
}

fn lines(c: viscas_plot::Curves) -> (Vec<Primitive>, bool) {
    let truncated = c.truncated;
    let prims = c
        .data
        .into_iter()
        .map(|line| Primitive::Polyline {
            line,
            style: Default::default(),
        })
        .collect();
    (prims, truncated)
}

fn region(r: viscas_plot::Region) -> (Vec<Primitive>, bool) {
    (r.primitives(), r.truncated)
}

/// Samples a shape over a view.
pub fn render(
    shape: &PlotShape,
    view: &View,
    opts: &PlotOptions,
) -> Result<(Vec<Primitive>, bool), Cancelled> {
    Ok(match shape {
        PlotShape::ExplicitY(f) => lines(plot_explicit(&**f, view, opts)?),
        PlotShape::ExplicitX(f) => lines(plot_explicit_x(&**f, view, opts)?),
        PlotShape::Parametric { x, y, range } => {
            let g = |t: f64| (x(t), y(t));
            lines(plot_parametric(&g, *range, view, opts)?)
        }
        PlotShape::Polar { r, range } => lines(plot_polar(&**r, *range, view, opts)?),
        PlotShape::PolarAngle { theta } => {
            let big = view_radius(view) * 1.05 + 1.0;
            let g = |r: f64| {
                let (s, c) = theta(r).sin_cos();
                (r * c, r * s)
            };
            lines(plot_parametric(&g, (-big, big), view, opts)?)
        }
        PlotShape::Implicit(f) => lines(plot_implicit(&**f, view, opts)?),
        PlotShape::InequalityY { f, cmp } => {
            region(plot_explicit_inequality(&**f, *cmp, view, opts)?)
        }
        PlotShape::InequalityX { f, cmp } => {
            region(plot_explicit_x_inequality(&**f, *cmp, view, opts)?)
        }
        PlotShape::InequalityXY { f, cmp } => {
            region(plot_implicit_inequality(&**f, *cmp, view, opts)?)
        }
        PlotShape::Points { points, connect } => {
            opts.cancel.check()?;
            (plot_point_list(points, *connect, view, opts), false)
        }
    })
}

/// Default parameter range of parametric and polar curves: `[0, 2π]`.
pub const DEFAULT_RANGE: (f64, f64) = (0.0, TAU);
