//! Scene plots in a viewport: per-viewport plot state (engine viewport,
//! cached layers, feature dots, box zoom), the cairo layer drawing it and
//! the pure helpers behind path building, hit testing and labels.
//!
//! Drawing rules:
//! - Stale layers (computed for an older view or revision) are drawn at
//!   full opacity until the fresh result arrives, on purpose: dimming them
//!   would flicker on every pan and edit.
//! - Fills use the even-odd rule, so regions with holes render correctly.
//! - Paths are built in screen space by [`polyline_ops`] and
//!   [`polygon_ops`]: segments are clipped (Liang–Barsky / Sutherland–
//!   Hodgman) to cairo's safe coordinate range instead of clamping
//!   endpoints, invisible segments of open lines are skipped, and the
//!   dash phase is carried across skipped parts.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, pango};
use viscas_engine::{FeatureHit, PlotLayer, ViewportId};
use viscas_plot::{FeatureKind, LineStyle, Primitive, Rect, Vec2};
use viscas_scene::{Color, KindTag, NodeId, Status};

use super::Layer;
use super::view::View;
use crate::ctx::AppCtx;
use crate::doc::Subscription;
use crate::theme::model::Color as ThemeColor;

/// Extra sampled area around the visible rectangle, per side (fraction of
/// its size), so small pans reuse the cached layers.
pub const PLOT_MARGIN: f64 = 0.25;
/// Pointer distance (px) within which a curve counts as hovered.
pub const HOVER_PX: f64 = 8.0;
/// Above this many feature dots, only the hovered one is labelled.
const MAX_DOT_LABELS: usize = 12;

/// Pointer distance (px) within which a feature dot counts as hovered.
pub const DOT_PX: f64 = 7.0;
/// Feature search radius around a click, in px.
pub const FEATURE_RADIUS_PX: f64 = 80.0;
/// Screen coordinates handed to cairo stay within ±this (fixed-point
/// limits); longer segments are clipped.
pub const COORD_LIMIT: f64 = 4.0e6;
/// Squared screen distance below which consecutive points are merged.
const MIN_STEP2: f64 = 0.9 * 0.9;
/// Screen margin (px) kept around the widget when culling segments.
const CULL_PAD: f64 = 16.0;

/// Two screen corners of a box-zoom rectangle.
pub type ScreenBox = ((f64, f64), (f64, f64));

/// Bounding boxes of every primitive of every layer (same order).
pub type LayerBoxes = Vec<Vec<Option<Rect>>>;

/// Style values the draw function needs, cached until the preferences
/// change (no per-frame theme clones or font parsing).
struct DrawStyle {
    antialias: bool,
    background: ThemeColor,
    tooltip_bg: ThemeColor,
    tooltip_fg: ThemeColor,
    accent: ThemeColor,
    font: pango::FontDescription,
    /// Created from the widget on first use.
    layout: Option<pango::Layout>,
}

/// Plot state of one viewport editor. Dropping it closes the engine
/// viewport.
pub struct PlotState {
    pub ctx: AppCtx,
    pub id: ViewportId,
    /// Layers of the last refresh (geometry shared with the engine cache).
    pub layers: RefCell<Vec<PlotLayer>>,
    /// World bounding box of every primitive of `layers` (same order),
    /// computed once per refresh so drawing and hit tests skip far ones.
    pub boxes: RefCell<LayerBoxes>,
    /// The padded view of the last plot request.
    pub requested: Cell<Option<viscas_plot::View>>,
    /// The current view cannot be sampled (degenerate or out of range).
    pub view_error: Cell<bool>,
    /// Feature dots of the last click.
    pub features: RefCell<Vec<FeatureHit>>,
    /// World point of the last feature request.
    pub click_world: Cell<Option<Vec2>>,
    /// Dots whose label stays visible.
    pub pinned: RefCell<Vec<FeatureHit>>,
    /// The dot under the pointer (its label shows).
    pub hover_dot: Cell<Option<Vec2>>,
    /// Box-zoom rectangle in screen coordinates.
    pub zoom_box: Cell<Option<ScreenBox>>,
    /// Error messages of plot nodes and failed layers.
    pub errors: RefCell<Vec<String>>,
    /// Layers cut short by the evaluation budget.
    pub truncated: Cell<usize>,
    /// A coalesced replot is queued.
    pub replot_queued: Cell<bool>,
    /// Pointer position of the last hover hit test.
    pub last_hover: Cell<Option<(f64, f64)>>,
    /// The current press turned into a drag (pan or box): its release
    /// must not select or clear.
    pub dragged: Cell<bool>,
    pub subscription: RefCell<Option<Subscription>>,
    style: RefCell<Option<DrawStyle>>,
}

impl PlotState {
    pub fn new(ctx: &AppCtx) -> Rc<Self> {
        let id = ctx.engine().create_viewport();
        Rc::new(Self {
            ctx: ctx.clone(),
            id,
            layers: RefCell::default(),
            boxes: RefCell::default(),
            requested: Cell::new(None),
            view_error: Cell::new(false),
            features: RefCell::default(),
            click_world: Cell::new(None),
            pinned: RefCell::default(),
            hover_dot: Cell::new(None),
            zoom_box: Cell::new(None),
            errors: RefCell::default(),
            truncated: Cell::new(0),
            replot_queued: Cell::new(false),
            last_hover: Cell::new(None),
            dragged: Cell::new(false),
            subscription: RefCell::default(),
            style: RefCell::new(None),
        })
    }

    /// Drops the cached draw style (preferences changed).
    pub fn invalidate_style(&self) {
        *self.style.borrow_mut() = None;
    }

    /// Re-reads the layers and error summary from the engine.
    pub fn refresh(&self) {
        let (layers, errors) = self.ctx.engine().with(|e| {
            let layers = e.layers(self.id);
            let scene = e.scene();
            let mut errors: Vec<String> = scene
                .iter()
                .filter(|n| matches!(n.kind.tag(), KindTag::Plot | KindTag::Point))
                .filter_map(|n| match scene.status(n.id) {
                    Status::Error { message, .. } => Some(format!(
                        "{}: {message}",
                        n.name.clone().unwrap_or_else(|| n.id.to_string())
                    )),
                    _ => None,
                })
                .collect();
            errors.extend(
                layers
                    .iter()
                    .filter_map(|l| l.error.as_ref().map(|m| format!("{}: {m}", l.node_id))),
            );
            (layers, errors)
        });
        self.truncated
            .set(layers.iter().filter(|l| l.truncated).count());
        *self.boxes.borrow_mut() = layers
            .iter()
            .map(|l| l.primitives.iter().map(primitive_box).collect())
            .collect();
        *self.layers.borrow_mut() = layers;
        *self.errors.borrow_mut() = errors;
        // Geometry changed: the next hover must hit-test again.
        self.last_hover.set(None);
    }

    /// Requests plots for `view` unless the cached layers still cover it.
    /// `force` re-sends the last padded view (scene changes). When the
    /// padded view is not representable the unpadded one is tried; when
    /// neither is, `view_error` is set (shown in the header chip).
    pub fn request(&self, view: &View, force: bool) {
        let covered = self.requested.get().is_some_and(|r| view.covered_by(&r));
        let target = if covered {
            if !force {
                return;
            }
            self.requested.get()
        } else {
            view.plot_view(PLOT_MARGIN).or_else(|| view.plot_view(0.0))
        };
        let allocated = view.size.0 >= 1.0 && view.size.1 >= 1.0;
        self.view_error.set(allocated && target.is_none());
        if let Some(target) = target {
            self.requested.set(Some(target));
            self.ctx.engine().request_plots(self.id, target);
        }
    }

    /// Clears feature dots, pins and the box.
    pub fn clear_marks(&self) {
        self.features.borrow_mut().clear();
        self.pinned.borrow_mut().clear();
        self.hover_dot.set(None);
        self.click_world.set(None);
    }

    /// The feature dot near a screen point (pinned ones first).
    pub fn dot_at(&self, view: &View, sx: f64, sy: f64) -> Option<FeatureHit> {
        let pinned = self.pinned.borrow();
        let features = self.features.borrow();
        pinned
            .iter()
            .chain(features.iter())
            .find(|h| {
                let (x, y) = view.world_to_screen(h.feature.pos.x, h.feature.pos.y);
                (x - sx).hypot(y - sy) <= DOT_PX
            })
            .copied()
    }

    /// Pins or unpins a dot's label.
    pub fn toggle_pin(&self, hit: FeatureHit) {
        let mut pinned = self.pinned.borrow_mut();
        if let Some(i) = pinned.iter().position(|p| same_spot(p, &hit)) {
            pinned.remove(i);
        } else {
            pinned.push(hit);
        }
    }
}

impl Drop for PlotState {
    fn drop(&mut self) {
        self.ctx.engine().close_viewport(self.id);
    }
}

fn same_spot(a: &FeatureHit, b: &FeatureHit) -> bool {
    (a.feature.pos - b.feature.pos).length() < 1e-9 && a.feature.kind == b.feature.kind
}

/// Feature hits within `radius_px` screen pixels of `center` (the search
/// itself uses a world radius, which is wider than intended along one
/// axis when the aspect is unlocked).
pub fn filter_hits(
    hits: &[FeatureHit],
    view: &View,
    center: Vec2,
    radius_px: f64,
) -> Vec<FeatureHit> {
    let c = view.world_to_screen(center.x, center.y);
    hits.iter()
        .filter(|h| {
            let s = view.world_to_screen(h.feature.pos.x, h.feature.pos.y);
            (s.0 - c.0).hypot(s.1 - c.1) <= radius_px
        })
        .copied()
        .collect()
}

/// Screen distance from `p` to segment `a`–`b`.
pub fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
}

/// The node whose curve or point passes nearest to the screen point,
/// within `max_px`. Fills are ignored (their boundary curves count).
/// `boxes` (from [`primitive_box`], same order as `layers`) lets far
/// primitives be skipped without looking at their points; a missing box
/// means "test it".
pub fn nearest_curve(
    layers: &[PlotLayer],
    boxes: &[Vec<Option<Rect>>],
    view: &View,
    p: (f64, f64),
    max_px: f64,
) -> Option<NodeId> {
    let mut best: Option<(f64, NodeId)> = None;
    // World box around the pointer.
    let (x0, y1) = view.screen_to_world(p.0 - max_px, p.1 - max_px);
    let (x1, y0) = view.screen_to_world(p.0 + max_px, p.1 + max_px);
    let near = Rect::new(x0, y0, x1, y1);
    let outside = |a: &Vec2, b: &Vec2| {
        (a.x < x0 && b.x < x0)
            || (a.x > x1 && b.x > x1)
            || (a.y < y0 && b.y < y0)
            || (a.y > y1 && b.y > y1)
    };
    for (li, layer) in layers.iter().enumerate() {
        if layer.error.is_some() {
            continue;
        }
        let mut consider = |d: f64| {
            if d <= max_px && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, layer.node_id));
            }
        };
        for (pi, prim) in layer.primitives.iter().enumerate() {
            match boxes.get(li).and_then(|b| b.get(pi)) {
                Some(None) => continue,
                Some(Some(b)) if !overlaps(b, &near) => continue,
                _ => {}
            }
            match prim {
                Primitive::Polyline { line, .. } => {
                    let pts = &line.points;
                    let closing = line.closed.then(|| (pts.last(), pts.first()));
                    let pairs = pts.windows(2).map(|w| (&w[0], &w[1]));
                    let extra = closing.and_then(|(a, b)| Some((a?, b?)));
                    for (a, b) in pairs.chain(extra) {
                        if !a.is_finite() || !b.is_finite() || outside(a, b) {
                            continue;
                        }
                        let sa = view.world_to_screen(a.x, a.y);
                        let sb = view.world_to_screen(b.x, b.y);
                        consider(segment_distance(p, sa, sb));
                    }
                }
                Primitive::Points(pts) => {
                    for q in pts.iter().filter(|q| q.is_finite() && !outside(q, q)) {
                        let s = view.world_to_screen(q.x, q.y);
                        consider((s.0 - p.0).hypot(s.1 - p.1));
                    }
                }
                Primitive::Polygon(_) => {}
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Short label of a feature kind.
pub fn feature_kind_label(kind: FeatureKind) -> &'static str {
    match kind {
        FeatureKind::Root => "root",
        FeatureKind::Minimum => "min",
        FeatureKind::Maximum => "max",
        FeatureKind::Intersection => "∩",
        FeatureKind::YIntercept => "y-int",
    }
}

/// A coordinate with up to 4 decimals, trailing zeros trimmed, `−` sign.
pub fn format_coord(v: f64) -> String {
    if !v.is_finite() {
        return "?".to_owned();
    }
    let v = if v.abs() < 5e-5 { 0.0 } else { v };
    let mut s = if v.abs() >= 1e6 {
        format!("{v:.3e}")
    } else {
        let mut s = format!("{v:.4}");
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
        s
    };
    if s == "-0" {
        s = "0".to_owned();
    }
    s.replace('-', "\u{2212}")
}

/// Label of a feature dot, e.g. `max (1.5708, 1)`.
pub fn feature_label(hit: &FeatureHit) -> String {
    format!(
        "{} ({}, {})",
        feature_kind_label(hit.feature.kind),
        format_coord(hit.feature.pos.x),
        format_coord(hit.feature.pos.y)
    )
}

/// Dash pattern (px) for a sampler line style.
pub fn dash_pattern(style: LineStyle) -> &'static [f64] {
    match style {
        LineStyle::Solid => &[],
        LineStyle::Dashed => &[7.0, 5.0],
    }
}

fn set_color(cr: &cairo::Context, c: Color, alpha: f64) {
    cr.set_source_rgba(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        f64::from(c.a) / 255.0 * alpha.clamp(0.0, 1.0),
    );
}

fn set_theme(cr: &cairo::Context, c: ThemeColor) {
    let (r, g, b, a) = c.rgba_f64();
    cr.set_source_rgba(r, g, b, a);
}

/// Cohen–Sutherland outcode of a screen point against the widget grown
/// by [`CULL_PAD`].
fn outcode(x: f64, y: f64, w: f64, h: f64) -> u8 {
    let mut c = 0;
    if x < -CULL_PAD {
        c |= 1;
    } else if x > w + CULL_PAD {
        c |= 2;
    }
    if y < -CULL_PAD {
        c |= 4;
    } else if y > h + CULL_PAD {
        c |= 8;
    }
    c
}

/// Liang–Barsky: parameter range of segment `a`→`b` inside the square
/// `[-lim, lim]²`.
pub fn clip_to_limit(a: (f64, f64), b: (f64, f64), lim: f64) -> Option<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 + lim),
        (dx, lim - a.0),
        (-dy, a.1 + lim),
        (dy, lim - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return None;
                }
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// One step of a screen-space path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathOp {
    /// Start a run at a point; the `f64` is the arc length (px) of the
    /// whole line up to it, to keep dashes in phase across gaps.
    Move(f64, f64, f64),
    Line(f64, f64),
}

fn lerp2(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Screen-space path of a polyline (closed: joined back to its first
/// point). Non-finite points break the line; segments are clipped to
/// [`COORD_LIMIT`]; with `cull`, segments entirely beside the widget are
/// skipped; steps shorter than 0.9 px are merged.
pub fn polyline_ops(
    view: &View,
    pts: &[Vec2],
    closed: bool,
    cull: bool,
    mut op: impl FnMut(PathOp),
) {
    let (w, h) = view.size;
    let closing = closed.then(|| pts.first()).flatten();
    let mut arc = 0.0;
    let mut prev: Option<((f64, f64), u8)> = None;
    let mut pen = false;
    let mut last = (f64::NAN, f64::NAN);
    // A merged point not yet emitted (flushed before the pen lifts).
    let mut pending: Option<(f64, f64)> = None;
    let flush = |pending: &mut Option<(f64, f64)>, op: &mut dyn FnMut(PathOp)| {
        if let Some((x, y)) = pending.take() {
            op(PathOp::Line(x, y));
        }
    };
    for q in pts.iter().chain(closing) {
        if !q.is_finite() {
            flush(&mut pending, &mut op);
            prev = None;
            pen = false;
            continue;
        }
        let s = view.world_to_screen(q.x, q.y);
        let code = if cull { outcode(s.0, s.1, w, h) } else { 0 };
        if let Some((p, pc)) = prev {
            let len = (s.0 - p.0).hypot(s.1 - p.1);
            let clipped = if pc & code != 0 {
                None
            } else {
                clip_to_limit(p, s, COORD_LIMIT)
            };
            match clipped {
                None => {
                    flush(&mut pending, &mut op);
                    pen = false;
                }
                Some((t0, t1)) => {
                    let a = lerp2(p, s, t0);
                    let b = lerp2(p, s, t1);
                    if !pen || t0 > 0.0 {
                        flush(&mut pending, &mut op);
                        op(PathOp::Move(a.0, a.1, arc + t0 * len));
                        pen = true;
                        last = a;
                    }
                    let (dx, dy) = (b.0 - last.0, b.1 - last.1);
                    if dx * dx + dy * dy >= MIN_STEP2 {
                        pending = None;
                        op(PathOp::Line(b.0, b.1));
                        last = b;
                    } else {
                        pending = Some(b);
                    }
                    if t1 < 1.0 {
                        flush(&mut pending, &mut op);
                        pen = false;
                    }
                }
            }
            arc += len;
        }
        prev = Some((s, code));
    }
    flush(&mut pending, &mut op);
}

/// Sutherland–Hodgman clip of a screen polygon to `[-lim, lim]²`.
pub fn clip_polygon(pts: &[(f64, f64)], lim: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = pts.to_vec();
    // (axis, sign): x ≥ -lim, x ≤ lim, y ≥ -lim, y ≤ lim.
    for (axis, sign) in [(0, -1.0), (0, 1.0), (1, -1.0), (1, 1.0)] {
        let inside = |p: &(f64, f64)| {
            let v = if axis == 0 { p.0 } else { p.1 };
            v * sign <= lim
        };
        let cross = |a: (f64, f64), b: (f64, f64)| {
            let (va, vb) = if axis == 0 { (a.0, b.0) } else { (a.1, b.1) };
            let t = (sign * lim - va) / (vb - va);
            lerp2(a, b, t)
        };
        let input = std::mem::take(&mut out);
        let Some(&first_prev) = input.last() else {
            return out;
        };
        let mut s = first_prev;
        for &e in &input {
            match (inside(&s), inside(&e)) {
                (true, true) => out.push(e),
                (true, false) => out.push(cross(s, e)),
                (false, true) => {
                    out.push(cross(s, e));
                    out.push(e);
                }
                (false, false) => {}
            }
            s = e;
        }
    }
    out
}

/// Screen-space path of a filled polygon (one closed run). Coordinates
/// beyond [`COORD_LIMIT`] are clipped as a polygon (allocates only then).
pub fn polygon_ops(view: &View, pts: &[Vec2], mut op: impl FnMut(PathOp)) {
    let finite = || pts.iter().filter(|q| q.is_finite());
    let in_range = finite().all(|q| {
        let s = view.world_to_screen(q.x, q.y);
        s.0.abs() <= COORD_LIMIT && s.1.abs() <= COORD_LIMIT
    });
    let mut emit = |it: &mut dyn Iterator<Item = (f64, f64)>| {
        let mut started = false;
        let mut last = (f64::NAN, f64::NAN);
        for (x, y) in it {
            if !started {
                op(PathOp::Move(x, y, 0.0));
                started = true;
                last = (x, y);
            } else if (x - last.0).powi(2) + (y - last.1).powi(2) >= MIN_STEP2 {
                op(PathOp::Line(x, y));
                last = (x, y);
            }
        }
    };
    if in_range {
        emit(&mut finite().map(|q| view.world_to_screen(q.x, q.y)));
    } else {
        let screen: Vec<(f64, f64)> = finite().map(|q| view.world_to_screen(q.x, q.y)).collect();
        emit(&mut clip_polygon(&screen, COORD_LIMIT).into_iter());
    }
}

/// Strokes a polyline, with dashes restarted in phase at each run.
fn stroke_polyline(cr: &cairo::Context, view: &View, pts: &[Vec2], closed: bool, dash: &[f64]) {
    // Closed lines are not culled (their join must stay intact).
    let cull = !closed;
    if dash.is_empty() {
        polyline_ops(view, pts, closed, cull, |o| match o {
            PathOp::Move(x, y, _) => cr.move_to(x, y),
            PathOp::Line(x, y) => cr.line_to(x, y),
        });
        let _ = cr.stroke();
        return;
    }
    let mut open = false;
    polyline_ops(view, pts, closed, cull, |o| match o {
        PathOp::Move(x, y, offset) => {
            if open {
                let _ = cr.stroke();
            }
            cr.set_dash(dash, offset);
            cr.move_to(x, y);
            open = true;
        }
        PathOp::Line(x, y) => cr.line_to(x, y),
    });
    let _ = cr.stroke();
}

/// The cairo layer drawing a viewport's plots, feature dots and box.
pub struct PlotsLayer {
    pub state: Rc<PlotState>,
}

impl PlotsLayer {
    /// The cached draw style, built from the preferences when missing.
    fn ensure_style(&self, widget: &gtk::Widget) {
        let st = &self.state;
        let mut style = st.style.borrow_mut();
        if style.is_none() {
            *style = Some(st.ctx.preferences().with(|p| {
                let theme = p.effective_theme();
                let mono = &p.interface.mono_font;
                let family = if mono.is_empty() {
                    "monospace".to_owned()
                } else {
                    mono.replace(',', " ")
                };
                DrawStyle {
                    antialias: p.viewport.antialias,
                    background: theme.viewport.background,
                    tooltip_bg: theme.ui.tooltip_bg,
                    tooltip_fg: theme.ui.tooltip_fg,
                    accent: theme.ui.accent,
                    font: pango::FontDescription::from_string(&format!("{family}, monospace 8.5")),
                    layout: None,
                }
            }));
        }
        if let Some(s) = style.as_mut()
            && s.layout.is_none()
        {
            let layout = widget.create_pango_layout(None);
            layout.set_font_description(Some(&s.font));
            s.layout = Some(layout);
        }
    }
}

impl Layer for PlotsLayer {
    fn draw(&self, widget: &gtk::Widget, cr: &cairo::Context, view: &View) {
        let st = &self.state;
        self.ensure_style(widget);
        let style = st.style.borrow();
        let Some(style) = style.as_ref() else {
            return;
        };
        cr.set_antialias(if style.antialias {
            cairo::Antialias::Default
        } else {
            cairo::Antialias::None
        });
        cr.set_line_join(cairo::LineJoin::Bevel);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        let sel = st.ctx.selection();
        let hovered = sel.hovered();
        let layers = st.layers.borrow();
        let boxes = st.boxes.borrow();
        let b = view.bounds();
        // Grow by the widest stroke so edges of thick lines are kept.
        let pad = 12.0 / view.scale.min(view.scale_y());
        let visible = Rect::new(b.x_min - pad, b.y_min - pad, b.x_max + pad, b.y_max + pad);
        for (i, layer) in layers.iter().enumerate().filter(|(_, l)| l.error.is_none()) {
            draw_layer(
                cr,
                view,
                layer,
                boxes.get(i).map_or(&[][..], |v| &v[..]),
                &visible,
                hovered == Some(layer.node_id),
                sel.is_selected(layer.node_id),
            );
        }
        cr.set_dash(&[], 0.0);

        // Feature dots and labels. Unpinned dots belong to the selection:
        // they go away with their curve's selection (B7).
        st.features.borrow_mut().retain(|h| sel.is_selected(h.node));
        let features = st.features.borrow();
        let pinned = st.pinned.borrow();
        let color_of = |id: NodeId| {
            layers
                .iter()
                .find(|l| l.node_id == id)
                .map(|l| l.style.color)
                .unwrap_or(Color::rgb(0xa0, 0xa8, 0xb8))
        };
        for hit in features.iter().chain(pinned.iter()) {
            let (x, y) = view.world_to_screen(hit.feature.pos.x, hit.feature.pos.y);
            cr.new_sub_path();
            cr.arc(x, y, 4.5, 0.0, TAU);
            set_theme(cr, style.background);
            let _ = cr.fill_preserve();
            set_color(cr, color_of(hit.node), 1.0);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
        // Every dot is labelled right away (`root (−0.5, 0)`); a crowded
        // set labels only the hovered dot.
        let hover_dot = st.hover_dot.get();
        let crowded = features.len() > MAX_DOT_LABELS;
        let labelled = |h: &&FeatureHit| {
            !crowded || hover_dot.is_some_and(|p| (p - h.feature.pos).length() < 1e-12)
        };
        if let Some(layout) = style.layout.as_ref() {
            for hit in pinned.iter().chain(features.iter().filter(labelled)) {
                layout.set_text(&feature_label(hit));
                let (tw, th) = layout.pixel_size();
                let (tw, th) = (f64::from(tw), f64::from(th));
                let (x, y) = view.world_to_screen(hit.feature.pos.x, hit.feature.pos.y);
                let (bx, by) = (x + 8.0, y - th - 10.0);
                let (bw, bh) = (tw + 12.0, th + 4.0);
                pill(cr, bx, by, bw, bh);
                set_theme(cr, style.tooltip_bg);
                let _ = cr.fill_preserve();
                set_color(cr, color_of(hit.node), 0.9);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
                set_theme(cr, style.tooltip_fg);
                cr.move_to(bx + 6.0, by + 2.0);
                pangocairo::functions::show_layout(cr, layout);
            }
        }

        // Box zoom.
        if let Some((a, b)) = st.zoom_box.get() {
            let (x, y) = (a.0.min(b.0), a.1.min(b.1));
            let (w, h) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
            cr.rectangle(x.floor() + 0.5, y.floor() + 0.5, w.round(), h.round());
            let (r, g, bl, _) = style.accent.rgba_f64();
            cr.set_source_rgba(r, g, bl, 0.12);
            let _ = cr.fill_preserve();
            cr.set_source_rgba(r, g, bl, 0.8);
            cr.set_line_width(1.0);
            cr.set_dash(&[4.0, 3.0], 0.0);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    }
}

/// World bounding box of a primitive (`None` when it has no finite point).
pub fn primitive_box(p: &Primitive) -> Option<Rect> {
    let pts: &[Vec2] = match p {
        Primitive::Polyline { line, .. } => &line.points,
        Primitive::Polygon(poly) => &poly.points,
        Primitive::Points(pts) => pts,
    };
    let mut it = pts.iter().filter(|q| q.is_finite());
    let first = it.next()?;
    let mut r = Rect::new(first.x, first.y, first.x, first.y);
    for q in it {
        r.x_min = r.x_min.min(q.x);
        r.x_max = r.x_max.max(q.x);
        r.y_min = r.y_min.min(q.y);
        r.y_max = r.y_max.max(q.y);
    }
    Some(r)
}

/// Whether two world rectangles overlap.
pub fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x_min <= b.x_max && b.x_min <= a.x_max && a.y_min <= b.y_max && b.y_min <= a.y_max
}

fn draw_layer(
    cr: &cairo::Context,
    view: &View,
    layer: &PlotLayer,
    boxes: &[Option<Rect>],
    visible: &Rect,
    hovered: bool,
    selected: bool,
) {
    let s = &layer.style;
    let base = s.line_width.clamp(0.5, 12.0);
    for (i, prim) in layer.primitives.iter().enumerate() {
        // No box: nothing finite to draw. Unknown (stale refresh): draw.
        match boxes.get(i) {
            Some(None) => continue,
            Some(Some(b)) if !overlaps(b, visible) => continue,
            _ => {}
        }
        match prim {
            Primitive::Polygon(poly) => {
                polygon_ops(view, &poly.points, |o| match o {
                    PathOp::Move(x, y, _) => cr.move_to(x, y),
                    PathOp::Line(x, y) => cr.line_to(x, y),
                });
                cr.close_path();
                set_color(
                    cr,
                    s.color,
                    s.fill_opacity * s.opacity * if hovered { 1.4 } else { 1.0 },
                );
                let _ = cr.fill();
            }
            Primitive::Polyline { line, style } => {
                // Node dash style wins over the sampler hint when set.
                let dash = match s.line_style {
                    viscas_scene::LineStyle::Solid => dash_pattern(*style),
                    viscas_scene::LineStyle::Dashed => dash_pattern(LineStyle::Dashed),
                    viscas_scene::LineStyle::Dotted => &[1.0, 4.0],
                };
                if selected {
                    cr.set_dash(&[], 0.0);
                    cr.set_line_width(base + 6.0);
                    set_color(cr, s.color, 0.22 * s.opacity);
                    stroke_polyline(cr, view, &line.points, line.closed, &[]);
                }
                cr.set_line_width(if hovered { base + 1.75 } else { base });
                set_color(cr, s.color, s.opacity);
                cr.set_dash(dash, 0.0);
                stroke_polyline(cr, view, &line.points, line.closed, dash);
                cr.set_dash(&[], 0.0);
            }
            Primitive::Points(pts) => {
                let r = s.point_size.clamp(1.0, 20.0) + if hovered { 1.5 } else { 0.0 };
                set_color(cr, s.color, s.opacity);
                for q in pts.iter().filter(|q| {
                    q.is_finite()
                        && q.x >= visible.x_min
                        && q.x <= visible.x_max
                        && q.y >= visible.y_min
                        && q.y <= visible.y_max
                }) {
                    let (x, y) = view.world_to_screen(q.x, q.y);
                    cr.new_sub_path();
                    cr.arc(x, y, r, 0.0, TAU);
                }
                let _ = cr.fill();
            }
        }
    }
}

/// Adds a pill-shaped path.
fn pill(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let r = h / 2.0;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -TAU / 4.0, TAU / 4.0);
    cr.arc(x + r, y + r, r, TAU / 4.0, 3.0 * TAU / 4.0);
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use viscas_engine::{PlotKind, PlotStyle};
    use viscas_plot::{Feature, Polyline};

    fn layer(node: u64, prims: Vec<Primitive>) -> PlotLayer {
        PlotLayer {
            node_id: NodeId(node),
            kind: PlotKind::ExplicitY,
            style: PlotStyle::from_style(&Default::default(), Color::rgb(1, 2, 3)),
            primitives: Arc::new(prims),
            truncated: false,
            error: None,
            stale: false,
        }
    }

    fn boxes_of(layers: &[PlotLayer]) -> LayerBoxes {
        layers
            .iter()
            .map(|l| l.primitives.iter().map(primitive_box).collect())
            .collect()
    }

    fn ops(view: &View, pts: &[(f64, f64)], closed: bool, cull: bool) -> Vec<PathOp> {
        let pts: Vec<Vec2> = pts.iter().map(|&(x, y)| Vec2::new(x, y)).collect();
        let mut out = Vec::new();
        polyline_ops(view, &pts, closed, cull, |o| out.push(o));
        out
    }

    fn line(pts: &[(f64, f64)]) -> Primitive {
        Primitive::Polyline {
            line: Polyline::open(pts.iter().map(|&(x, y)| Vec2::new(x, y)).collect()),
            style: LineStyle::Solid,
        }
    }

    #[test]
    fn segment_distance_projects_and_clamps() {
        assert_eq!(segment_distance((5.0, 3.0), (0.0, 0.0), (10.0, 0.0)), 3.0);
        assert_eq!(segment_distance((-4.0, 3.0), (0.0, 0.0), (10.0, 0.0)), 5.0);
        assert_eq!(segment_distance((1.0, 1.0), (1.0, 2.0), (1.0, 2.0)), 1.0);
    }

    #[test]
    fn nearest_curve_picks_the_closest_within_reach() {
        let view = View::default(); // 60 px/unit, origin at (400, 300)
        let layers = vec![
            layer(1, vec![line(&[(-5.0, 0.0), (5.0, 0.0)])]),
            layer(2, vec![line(&[(-5.0, 1.0), (5.0, 1.0)])]),
            layer(3, vec![Primitive::Points(vec![Vec2::new(3.0, -2.0)])]),
        ];
        // 5 px below the x axis.
        assert_eq!(
            nearest_curve(&layers, &boxes_of(&layers), &view, (400.0, 305.0), HOVER_PX),
            Some(NodeId(1))
        );
        // 4 px below y = 1 (screen y 240).
        assert_eq!(
            nearest_curve(&layers, &boxes_of(&layers), &view, (420.0, 244.0), HOVER_PX),
            Some(NodeId(2))
        );
        assert_eq!(
            nearest_curve(&layers, &boxes_of(&layers), &view, (580.0, 422.0), HOVER_PX),
            Some(NodeId(3))
        );
        assert_eq!(
            nearest_curve(&layers, &boxes_of(&layers), &view, (400.0, 270.0), HOVER_PX),
            None
        );
        let mut broken = layers.clone();
        broken[0].error = Some("boom".into());
        assert_eq!(
            nearest_curve(&broken, &boxes_of(&broken), &view, (400.0, 305.0), HOVER_PX),
            None
        );
    }

    #[test]
    fn coordinates_and_labels() {
        assert_eq!(format_coord(1.234_567), "1.2346");
        assert_eq!(format_coord(2.0), "2");
        assert_eq!(format_coord(-0.00001), "0");
        assert_eq!(format_coord(-1.25), "\u{2212}1.25");
        assert_eq!(format_coord(f64::NAN), "?");
        let hit = FeatureHit {
            node: NodeId(1),
            other: None,
            feature: Feature {
                pos: Vec2::new(1.5, -1.0),
                kind: FeatureKind::Maximum,
            },
            distance: 0.0,
        };
        assert_eq!(feature_label(&hit), "max (1.5, \u{2212}1)");
    }

    #[test]
    fn boxes_and_overlap() {
        let b = primitive_box(&line(&[(1.0, 2.0), (f64::NAN, 0.0), (-1.0, 5.0)])).unwrap();
        assert_eq!((b.x_min, b.y_min, b.x_max, b.y_max), (-1.0, 2.0, 1.0, 5.0));
        assert!(primitive_box(&Primitive::Points(vec![])).is_none());
        let v = viscas_plot::Rect::new(0.0, 0.0, 1.0, 3.0);
        assert!(overlaps(&b, &v));
        assert!(!overlaps(&viscas_plot::Rect::new(2.0, 2.0, 3.0, 3.0), &v));
    }

    #[test]
    fn dashes() {
        assert!(dash_pattern(LineStyle::Solid).is_empty());
        assert_eq!(dash_pattern(LineStyle::Dashed).len(), 2);
    }

    #[test]
    fn nearest_curve_skips_far_boxes_and_tests_unboxed() {
        let view = View::default();
        let layers = vec![layer(1, vec![line(&[(-5.0, 0.0), (5.0, 0.0)])])];
        // A wrong (far) box hides the curve: proof the prefilter is used.
        let far = vec![vec![Some(Rect::new(50.0, 50.0, 60.0, 60.0))]];
        assert_eq!(
            nearest_curve(&layers, &far, &view, (400.0, 305.0), HOVER_PX),
            None
        );
        // No boxes at all: everything is tested.
        assert_eq!(
            nearest_curve(&layers, &[], &view, (400.0, 305.0), HOVER_PX),
            Some(NodeId(1))
        );
    }

    #[test]
    fn huge_segments_are_clipped_not_clamped() {
        // Zoomed far in: the segment's ends are ~1e9 px away but it passes
        // through the screen; its direction must survive.
        let view = View {
            scale: 1e6,
            ..View::default()
        };
        let o = ops(&view, &[(-1000.0, -1000.0), (1000.0, 1000.0)], false, false);
        let [PathOp::Move(ax, ay, _), PathOp::Line(bx, by)] = o[..] else {
            panic!("{o:?}");
        };
        for v in [ax, ay, bx, by] {
            assert!(v.abs() <= COORD_LIMIT * 1.000_001);
        }
        // Still the 45° screen diagonal through the center (400, 300).
        assert!(((ay - 300.0) + (ax - 400.0)).abs() < 1e-3 * COORD_LIMIT);
        assert!(((by - 300.0) + (bx - 400.0)).abs() < 1e-3 * COORD_LIMIT);
        assert_eq!(clip_to_limit((0.0, 5e6), (1.0, 5e6), COORD_LIMIT), None);
        assert_eq!(
            clip_to_limit((0.0, 0.0), (1.0, 1.0), COORD_LIMIT),
            Some((0.0, 1.0))
        );
    }

    #[test]
    fn culled_runs_carry_the_dash_phase() {
        let view = View::default(); // 60 px/unit, x in about [-6.7, 6.7]
        // Visible, far off to the right (skipped), visible again.
        let o = ops(
            &view,
            &[
                (0.0, 0.0),
                (1.0, 0.0),
                (100.0, 0.0),
                (200.0, 0.0),
                (1.0, 0.5),
                (0.0, 0.5),
            ],
            false,
            true,
        );
        let moves: Vec<f64> = o
            .iter()
            .filter_map(|p| match p {
                PathOp::Move(_, _, off) => Some(*off),
                _ => None,
            })
            .collect();
        assert_eq!(moves.len(), 2, "{o:?}");
        assert_eq!(moves[0], 0.0);
        // The second run starts at (200, 0) after 200 world units of line:
        // the skipped (100 → 200) part still counts.
        assert!((moves[1] - 60.0 * 200.0).abs() < 1e-6, "{}", moves[1]);
    }

    #[test]
    fn tiny_steps_are_merged_but_ends_kept() {
        let view = View::default();
        let pts: Vec<(f64, f64)> = (0..=100).map(|i| (f64::from(i) * 0.001, 0.0)).collect();
        let o = ops(&view, &pts, false, false);
        // 0.06 px per step: merged, but the final point is kept.
        assert!(o.len() < 15, "{}", o.len());
        let Some(PathOp::Line(x, _)) = o.last() else {
            panic!("{o:?}");
        };
        assert!((x - 406.0).abs() < 1e-9);
        // Non-finite points break the line.
        let o = ops(
            &view,
            &[
                (0.0, 0.0),
                (1.0, 0.0),
                (f64::NAN, 0.0),
                (2.0, 0.0),
                (3.0, 0.0),
            ],
            false,
            false,
        );
        assert_eq!(
            o.iter().filter(|p| matches!(p, PathOp::Move(..))).count(),
            2
        );
    }

    #[test]
    fn polygons_clip_to_the_limit() {
        let sq = [(-1e7, -1e7), (1e7, -1e7), (1e7, 1e7), (-1e7, 1e7)];
        let c = clip_polygon(&sq, COORD_LIMIT);
        assert_eq!(c.len(), 4);
        for (x, y) in c {
            assert!(x.abs() <= COORD_LIMIT && y.abs() <= COORD_LIMIT);
        }
        assert!(clip_polygon(&[(5e6, 5e6), (6e6, 5e6), (6e6, 6e6)], COORD_LIMIT).is_empty());
        // In range: one run, unchanged count.
        let view = View::default();
        let tri = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
        ];
        let mut n = 0;
        polygon_ops(&view, &tri, |_| n += 1);
        assert_eq!(n, 3);
    }

    #[test]
    fn feature_hits_filter_by_screen_distance() {
        let hit = |x: f64, y: f64| FeatureHit {
            node: NodeId(1),
            other: None,
            feature: Feature {
                pos: Vec2::new(x, y),
                kind: FeatureKind::Root,
            },
            distance: 0.0,
        };
        // Free aspect: y is stretched 10x, so (0, 1) is 600 px away.
        let view = View {
            aspect: 10.0,
            ..View::default()
        };
        let hits = [hit(1.0, 0.0), hit(0.0, 1.0)];
        let kept = filter_hits(&hits, &view, Vec2::new(0.0, 0.0), FEATURE_RADIUS_PX);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].feature.pos, Vec2::new(1.0, 0.0));
    }

    /// Alpha of the pixel at (x, y) after drawing `layer` on an 800×600
    /// transparent surface with the default view.
    fn render(layer: &PlotLayer) -> impl Fn(i32, i32) -> u8 {
        let view = View::default();
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 800, 600).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            let boxes: Vec<Option<Rect>> = layer.primitives.iter().map(primitive_box).collect();
            let b = view.bounds();
            let visible = Rect::new(b.x_min, b.y_min, b.x_max, b.y_max);
            draw_layer(&cr, &view, layer, &boxes, &visible, false, false);
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap().to_vec();
        // ARGB32 is native-endian; alpha is the high byte.
        move |x: i32, y: i32| {
            let i = y as usize * stride + x as usize * 4;
            let px = u32::from_ne_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            (px >> 24) as u8
        }
    }

    #[test]
    fn draws_lines_dashes_and_even_odd_fills() {
        // Solid line along y = 1 (screen row 240), extending far off-screen.
        let solid = layer(1, vec![line(&[(-1e4, 1.0), (1e4, 1.0)])]);
        let a = render(&solid);
        assert!(a(400, 240) > 200 && a(10, 240) > 200);
        assert_eq!(a(400, 200), 0);

        // Dashed: some pixels along the row are empty.
        let dashed = layer(
            2,
            vec![Primitive::Polyline {
                line: Polyline::open(vec![Vec2::new(-5.0, 1.0), Vec2::new(5.0, 1.0)]),
                style: LineStyle::Dashed,
            }],
        );
        let d = render(&dashed);
        let gaps = (200..600).filter(|x| d(*x, 240) == 0).count();
        assert!(gaps > 50 && gaps < 300, "{gaps}");

        // A square ring as one polygon traced outer then inner: even-odd
        // leaves the hole empty.
        let ring = Primitive::Polygon(viscas_plot::Polygon::new(vec![
            Vec2::new(-2.0, -2.0),
            Vec2::new(2.0, -2.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(-2.0, 2.0),
            Vec2::new(-2.0, -2.0),
            Vec2::new(-1.0, -1.0),
            Vec2::new(-1.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(-1.0, -1.0),
        ]));
        let mut l = layer(3, vec![ring]);
        l.style.fill_opacity = 1.0;
        let f = render(&l);
        assert!(f(400 + 90, 300) > 200, "ring filled");
        assert_eq!(f(400, 300), 0, "hole empty");
    }
}
