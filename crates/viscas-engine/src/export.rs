//! Headless rendering for export: re-samples plots at export density over
//! an arbitrary world box, optionally restricted to part of a curve or
//! clipped to a box, off the UI thread and cancellable.
//!
//! ```
//! use std::sync::Arc;
//! use viscas_engine::export::{ExportItem, ExportJob, ExportRange};
//! use viscas_engine::{CompiledPlot, PlotItem, PlotKind, PlotShape, PlotStyle};
//! use viscas_plot::Rect;
//! use viscas_scene::{Color, NodeId};
//!
//! let plot = CompiledPlot::new(PlotKind::ExplicitY, PlotShape::ExplicitY(Arc::new(|x| x)));
//! let item = PlotItem {
//!     node: NodeId(1),
//!     revision: 0,
//!     style: PlotStyle::from_style(&Default::default(), Color::rgb(255, 0, 0)),
//!     plot: Arc::new(plot),
//! };
//! let job = ExportJob::new(
//!     vec![ExportItem::new(item).with_range(ExportRange::Param(0.0, 1.0))],
//!     Rect::new(-2.0, -2.0, 2.0, 2.0),
//!     (400.0, 400.0),
//!     1.0,
//! );
//! let layers = job.run(|_, _| {}).unwrap();
//! assert_eq!(layers.len(), 1);
//! ```

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use viscas_plot::{
    CancelToken, Cancelled, PlotOptions, Primitive, Rect, Vec2, View, plot_explicit_on,
    sample_explicit_x,
};

use crate::plot::{PlotItem, PlotLayer, PlotShape, render};

/// Which part of a node to export.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ExportRange {
    /// Everything inside the export box.
    #[default]
    Full,
    /// Only `x ∈ [a, b]` (any plot kind; geometry is cut at the bounds).
    X(f64, f64),
    /// The curve's own parameter in `[a, b]`: `x` for `y = f(x)`, `y` for
    /// `x = f(y)`, `t` for parametric curves, `θ` for polar curves. Other
    /// kinds fall back to [`ExportRange::X`].
    Param(f64, f64),
}

impl ExportRange {
    fn ordered(a: f64, b: f64) -> Option<(f64, f64)> {
        (a.is_finite() && b.is_finite() && a != b).then(|| (a.min(b), a.max(b)))
    }
}

/// One node of an [`ExportJob`].
#[derive(Clone, Debug)]
pub struct ExportItem {
    pub item: PlotItem,
    pub range: ExportRange,
    /// Clip the result to this world box (e.g. an inequality region).
    pub clip: Option<Rect>,
}

impl ExportItem {
    pub fn new(item: PlotItem) -> ExportItem {
        ExportItem {
            item,
            range: ExportRange::Full,
            clip: None,
        }
    }

    pub fn with_range(mut self, range: ExportRange) -> ExportItem {
        self.range = range;
        self
    }

    pub fn with_clip(mut self, clip: Option<Rect>) -> ExportItem {
        self.clip = clip;
        self
    }
}

/// Largest evaluation budget per curve for export renders.
const MAX_EXPORT_EVALS: usize = 4_000_000;

/// A `Send` export render: items, the world box and a sampling density.
#[derive(Clone, Debug)]
pub struct ExportJob {
    pub items: Vec<ExportItem>,
    pub world: Rect,
    /// Sampling canvas in pixels (output size × quality).
    pub size_px: (f64, f64),
    pub options: PlotOptions,
    flag: Arc<AtomicBool>,
}

impl ExportJob {
    /// A job rendering `items` over `world` for an output of `size_px`
    /// pixels, sampled `quality` times denser (1 = screen density).
    pub fn new(items: Vec<ExportItem>, world: Rect, size_px: (f64, f64), quality: f64) -> Self {
        let q = if quality.is_finite() {
            quality.clamp(0.25, 16.0)
        } else {
            1.0
        };
        let flag = Arc::new(AtomicBool::new(false));
        let defaults = PlotOptions::default();
        let options = PlotOptions {
            // Exact clip: the export box is the page.
            clip_margin: 0.0,
            max_evals: ((defaults.max_evals as f64 * q * q) as usize)
                .clamp(defaults.max_evals / 4, MAX_EXPORT_EVALS),
            parametric_samples: ((defaults.parametric_samples as f64 * q) as usize).max(64),
            cancel: CancelToken::from(flag.clone()),
            ..defaults
        };
        let size_px = (
            (size_px.0 * q).clamp(16.0, 40_000.0),
            (size_px.1 * q).clamp(16.0, 40_000.0),
        );
        let items = items
            .into_iter()
            .map(|mut it| {
                it.item.plot = it.item.plot.for_job(&flag);
                it
            })
            .collect();
        ExportJob {
            items,
            world,
            size_px,
            options,
            flag,
        }
    }

    /// Token cancelling this job (from any thread).
    pub fn cancel_token(&self) -> CancelToken {
        CancelToken::from(self.flag.clone())
    }

    /// Renders every item; `progress(done, total)` runs after each one.
    pub fn run(&self, mut progress: impl FnMut(usize, usize)) -> Result<Vec<PlotLayer>, Cancelled> {
        let view =
            View::try_new(self.world, self.size_px.0, self.size_px.1).map_err(|_| Cancelled)?;
        let total = self.items.len();
        let mut out = Vec::with_capacity(total);
        for (i, it) in self.items.iter().enumerate() {
            self.options.cancel.check()?;
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render_range(&it.item.plot.shape, it.range, &view, &self.options)
            }));
            let (mut prims, truncated, error) = match rendered {
                Ok(r) => {
                    let (p, t) = r?;
                    (p, t, None)
                }
                Err(_) => (
                    Vec::new(),
                    false,
                    Some("export failed (internal error)".into()),
                ),
            };
            if let Some(clip) = it.clip {
                prims = clip_primitives(&prims, &clip);
            }
            prims = clip_primitives(&prims, &self.world);
            self.options.cancel.check()?;
            out.push(PlotLayer {
                node_id: it.item.node,
                kind: it.item.plot.kind,
                style: it.item.style.clone(),
                primitives: Arc::new(prims),
                truncated,
                error,
                stale: false,
            });
            progress(i + 1, total);
        }
        Ok(out)
    }
}

/// Renders `shape` over `view` restricted to `range`.
pub fn render_range(
    shape: &PlotShape,
    range: ExportRange,
    view: &View,
    opts: &PlotOptions,
) -> Result<(Vec<Primitive>, bool), Cancelled> {
    let lines = |c: viscas_plot::Curves, clip: Rect| -> (Vec<Primitive>, bool) {
        let prims = c
            .data
            .iter()
            .flat_map(|l| l.clipped(&clip))
            .map(|line| Primitive::Polyline {
                line,
                style: Default::default(),
            })
            .collect();
        (prims, c.truncated)
    };
    let bounds = view.bounds();
    match (range, shape) {
        (ExportRange::Full, _) => render(shape, view, opts),
        (ExportRange::Param(a, b) | ExportRange::X(a, b), PlotShape::ExplicitY(f)) => {
            let Some(r) = ExportRange::ordered(a, b) else {
                return Ok((Vec::new(), false));
            };
            let c = plot_explicit_on(&**f, r, view, opts)?;
            Ok(lines(c, bounds))
        }
        (ExportRange::Param(a, b), PlotShape::ExplicitX(f)) => {
            let Some((lo, hi)) = ExportRange::ordered(a, b) else {
                return Ok((Vec::new(), false));
            };
            let (lo, hi) = (lo.max(bounds.y_min), hi.min(bounds.y_max));
            if lo >= hi {
                return Ok((Vec::new(), false));
            }
            let c = sample_explicit_x(&**f, (lo, hi), view, opts)?;
            Ok(lines(c, bounds))
        }
        (ExportRange::Param(a, b), PlotShape::Parametric { x, y, .. }) => {
            let Some(r) = ExportRange::ordered(a, b) else {
                return Ok((Vec::new(), false));
            };
            let restricted = PlotShape::Parametric {
                x: x.clone(),
                y: y.clone(),
                range: r,
            };
            render(&restricted, view, opts)
        }
        (ExportRange::Param(a, b), PlotShape::Polar { r, .. }) => {
            let Some(range) = ExportRange::ordered(a, b) else {
                return Ok((Vec::new(), false));
            };
            let restricted = PlotShape::Polar {
                r: r.clone(),
                range,
            };
            render(&restricted, view, opts)
        }
        (ExportRange::Param(a, b) | ExportRange::X(a, b), _) => {
            let Some((lo, hi)) = ExportRange::ordered(a, b) else {
                return Ok((Vec::new(), false));
            };
            let (prims, t) = render(shape, view, opts)?;
            Ok((trim_x(&prims, lo, hi), t))
        }
    }
}

/// Cuts primitives to the vertical slab `x ∈ [lo, hi]`.
pub fn trim_x(prims: &[Primitive], lo: f64, hi: f64) -> Vec<Primitive> {
    // Finite stand-ins for ±∞ keep the clipping arithmetic exact.
    const BIG: f64 = 1e300;
    clip_primitives(prims, &Rect::new(lo, -BIG, hi, BIG))
}

/// Clips primitives to `rect`: polylines split where they leave it,
/// polygons are cut (Sutherland–Hodgman), points outside are dropped.
pub fn clip_primitives(prims: &[Primitive], rect: &Rect) -> Vec<Primitive> {
    let mut out = Vec::with_capacity(prims.len());
    for p in prims {
        match p {
            Primitive::Polyline { line, style } => {
                out.extend(
                    line.clipped(rect)
                        .into_iter()
                        .map(|line| Primitive::Polyline {
                            line,
                            style: *style,
                        }),
                );
            }
            Primitive::Polygon(poly) => {
                if let Some(c) = poly.clipped(rect) {
                    out.push(Primitive::Polygon(c));
                }
            }
            Primitive::Points(pts) => {
                let kept: Vec<Vec2> = pts
                    .iter()
                    .copied()
                    .filter(|q| q.is_finite() && rect.contains(*q))
                    .collect();
                if !kept.is_empty() {
                    out.push(Primitive::Points(kept));
                }
            }
        }
    }
    out
}

/// The point of a curve at parameter `t` (see [`ExportRange::Param`]), or
/// `None` for shapes without a parameter.
pub fn point_at(shape: &PlotShape, t: f64) -> Option<Vec2> {
    let p = match shape {
        PlotShape::ExplicitY(f) => Vec2::new(t, f(t)),
        PlotShape::ExplicitX(f) => Vec2::new(f(t), t),
        PlotShape::Parametric { x, y, .. } => Vec2::new(x(t), y(t)),
        PlotShape::Polar { r, .. } => {
            let rr = r(t);
            let (s, c) = t.sin_cos();
            Vec2::new(rr * c, rr * s)
        }
        _ => return None,
    };
    Some(p)
}

/// The natural parameter range of a shape within `world`: the visible x
/// (or y) span for explicit curves, the curve's range otherwise.
pub fn param_range(shape: &PlotShape, world: &Rect) -> Option<(f64, f64)> {
    match shape {
        PlotShape::ExplicitY(_) => Some((world.x_min, world.x_max)),
        PlotShape::ExplicitX(_) => Some((world.y_min, world.y_max)),
        PlotShape::Parametric { range, .. } | PlotShape::Polar { range, .. } => Some(*range),
        _ => None,
    }
}

/// Name of the parameter of a shape (`x`, `y`, `t`, `θ`).
pub fn param_name(shape: &PlotShape) -> Option<&'static str> {
    match shape {
        PlotShape::ExplicitY(_) => Some("x"),
        PlotShape::ExplicitX(_) => Some("y"),
        PlotShape::Parametric { .. } => Some("t"),
        PlotShape::Polar { .. } => Some("θ"),
        _ => None,
    }
}

/// Parameter of the point of the curve nearest to `p` in `[a, b]`:
/// `samples` coarse uniform probes, then two local refinements around the
/// best probe (distances measured with `scale` = world units per pixel on
/// each axis).
pub fn nearest_param(
    shape: &PlotShape,
    (a, b): (f64, f64),
    p: Vec2,
    scale: (f64, f64),
    samples: usize,
) -> Option<f64> {
    const REFINE: usize = 32;
    let dist = |t: f64| {
        point_at(shape, t)
            .filter(|q| q.is_finite())
            .map(|q| ((q.x - p.x) / scale.0).hypot((q.y - p.y) / scale.1))
    };
    let probe = |lo: f64, hi: f64, n: usize| {
        let mut best: Option<(f64, f64)> = None;
        for i in 0..=n {
            let t = lo + (hi - lo) * i as f64 / n as f64;
            if let Some(d) = dist(t)
                && best.is_none_or(|(bd, _)| d < bd)
            {
                best = Some((d, t));
            }
        }
        best
    };
    if !(a.is_finite() && b.is_finite()) {
        return None;
    }
    let n = samples.max(2);
    let (mut best_d, mut t) = probe(a, b, n)?;
    let mut half = (b - a) / n as f64;
    for _ in 0..2 {
        let (lo, hi) = ((t - half).max(a.min(b)), (t + half).min(a.max(b)));
        if let Some((d, tt)) = probe(lo, hi, REFINE)
            && d <= best_d
        {
            best_d = d;
            t = tt;
        }
        half = (hi - lo) / REFINE as f64;
    }
    Some(t)
}

/// Uniform samples `(t, x, y)` of a curve for `t ∈ [a, b]`, split into
/// pieces where it is undefined. `None` for shapes without a parameter.
pub fn sample_table(shape: &PlotShape, (a, b): (f64, f64), n: usize) -> Option<Vec<Vec<[f64; 3]>>> {
    param_name(shape)?;
    let n = n.clamp(2, 10_000_000);
    let mut pieces: Vec<Vec<[f64; 3]>> = Vec::new();
    let mut cur: Vec<[f64; 3]> = Vec::new();
    for i in 0..n {
        let t = a + (b - a) * i as f64 / (n - 1) as f64;
        match point_at(shape, t).filter(|q| q.is_finite()) {
            Some(q) => cur.push([t, q.x, q.y]),
            None => {
                if !cur.is_empty() {
                    pieces.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        pieces.push(cur);
    }
    Some(pieces)
}

/// Bounding box of the finite points of some primitives.
pub fn primitives_bounds<'a>(prims: impl IntoIterator<Item = &'a Primitive>) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for p in prims {
        let pts: &[Vec2] = match p {
            Primitive::Polyline { line, .. } => &line.points,
            Primitive::Polygon(poly) => &poly.points,
            Primitive::Points(pts) => pts,
        };
        for q in pts.iter().filter(|q| q.is_finite()) {
            r = Some(match r {
                None => Rect::new(q.x, q.y, q.x, q.y),
                Some(r) => Rect::new(
                    r.x_min.min(q.x),
                    r.y_min.min(q.y),
                    r.x_max.max(q.x),
                    r.y_max.max(q.y),
                ),
            });
        }
    }
    r
}
