//! Scene plots in a viewport: per-viewport plot state (engine viewport,
//! cached layers, feature dots, box zoom), the cairo layer drawing it and
//! the pure helpers behind hit testing and labels.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, pango};
use viscas_engine::{FeatureHit, PlotLayer, ViewportId};
use viscas_plot::{FeatureKind, LineStyle, Primitive, Vec2};
use viscas_scene::{Color, KindTag, NodeId, Status};

use super::Layer;
use super::view::View;
use crate::ctx::AppCtx;
use crate::doc::Subscription;

/// Extra sampled area around the visible rectangle, per side (fraction of
/// its size), so small pans reuse the cached layers.
pub const PLOT_MARGIN: f64 = 0.25;
/// Pointer distance (px) within which a curve counts as hovered.
pub const HOVER_PX: f64 = 8.0;
/// Pointer distance (px) within which a feature dot counts as hovered.
pub const DOT_PX: f64 = 7.0;
/// Feature search radius around a click, in px.
pub const FEATURE_RADIUS_PX: f64 = 80.0;
/// Cairo coordinates are clamped to this range (fixed-point limits).
const COORD_LIMIT: f64 = 4.0e6;

/// Two screen corners of a box-zoom rectangle.
pub type ScreenBox = ((f64, f64), (f64, f64));

/// Plot state of one viewport editor. Dropping it closes the engine
/// viewport.
pub struct PlotState {
    pub ctx: AppCtx,
    pub id: ViewportId,
    /// Layers of the last refresh (geometry shared with the engine cache).
    pub layers: RefCell<Vec<PlotLayer>>,
    /// World bounding box of every primitive of `layers` (same order),
    /// computed once per refresh so drawing can skip off-screen ones.
    pub boxes: RefCell<Vec<Vec<Option<viscas_plot::Rect>>>>,
    /// The padded view of the last plot request.
    pub requested: Cell<Option<viscas_plot::View>>,
    /// Feature dots of the last click.
    pub features: RefCell<Vec<FeatureHit>>,
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
    pub subscription: RefCell<Option<Subscription>>,
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
            features: RefCell::default(),
            pinned: RefCell::default(),
            hover_dot: Cell::new(None),
            zoom_box: Cell::new(None),
            errors: RefCell::default(),
            truncated: Cell::new(0),
            replot_queued: Cell::new(false),
            subscription: RefCell::default(),
        })
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
    }

    /// Requests plots for `view` unless the cached layers still cover it.
    /// `force` re-sends the last padded view (scene changes).
    pub fn request(&self, view: &View, force: bool) {
        let covered = self.requested.get().is_some_and(|r| view.covered_by(&r));
        let target = if covered {
            if !force {
                return;
            }
            self.requested.get()
        } else {
            view.plot_view(PLOT_MARGIN)
        };
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
pub fn nearest_curve(
    layers: &[PlotLayer],
    view: &View,
    p: (f64, f64),
    max_px: f64,
) -> Option<NodeId> {
    let mut best: Option<(f64, NodeId)> = None;
    // World box around the pointer, to skip far segments cheaply.
    let (x0, y1) = view.screen_to_world(p.0 - max_px, p.1 - max_px);
    let (x1, y0) = view.screen_to_world(p.0 + max_px, p.1 + max_px);
    let outside = |a: &Vec2, b: &Vec2| {
        (a.x < x0 && b.x < x0)
            || (a.x > x1 && b.x > x1)
            || (a.y < y0 && b.y < y0)
            || (a.y > y1 && b.y > y1)
    };
    for layer in layers.iter().filter(|l| l.error.is_none()) {
        let mut consider = |d: f64| {
            if d <= max_px && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, layer.node_id));
            }
        };
        for prim in layer.primitives.iter() {
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
                    for q in pts {
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

fn clamp_coord(v: f64) -> f64 {
    v.clamp(-COORD_LIMIT, COORD_LIMIT)
}

fn set_color(cr: &cairo::Context, c: Color, alpha: f64) {
    cr.set_source_rgba(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        f64::from(c.a) / 255.0 * alpha.clamp(0.0, 1.0),
    );
}

fn set_theme(cr: &cairo::Context, c: crate::theme::model::Color) {
    let (r, g, b, a) = c.rgba_f64();
    cr.set_source_rgba(r, g, b, a);
}

/// Squared screen distance below which consecutive points are merged.
const MIN_STEP2: f64 = 0.9 * 0.9;
/// Screen margin (px) kept around the widget when culling segments.
const CULL_PAD: f64 = 16.0;

/// Cohen–Sutherland outcode of a screen point against `[-pad, w+pad] ×
/// [-pad, h+pad]`.
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

/// Adds a polyline path in screen space. Points closer than 0.9 px
/// to the previous one are merged and, for open lines (`cull`), segments
/// entirely off one side of the widget are skipped (the pen lifts), so
/// stroking cost follows what is visible.
fn path_polyline(cr: &cairo::Context, view: &View, pts: &[Vec2], closed: bool, cull: bool) {
    let (w, h) = view.size;
    let mut pen = false;
    // Last point added to the path, and the last point seen.
    let mut last = (f64::NAN, f64::NAN);
    let mut prev: Option<(f64, f64, u8)> = None;
    let n = pts.len();
    for (i, q) in pts.iter().enumerate() {
        if !q.is_finite() {
            pen = false;
            prev = None;
            continue;
        }
        let (x, y) = view.world_to_screen(q.x, q.y);
        let (x, y) = (clamp_coord(x), clamp_coord(y));
        let code = if cull { outcode(x, y, w, h) } else { 0 };
        if let Some((px, py, pc)) = prev
            && pc & code != 0
        {
            // Invisible segment: lift the pen.
            pen = false;
            prev = Some((x, y, code));
            let _ = (px, py);
            continue;
        }
        if !pen {
            if let Some((px, py, _)) = prev {
                cr.move_to(px, py);
                last = (px, py);
                pen = true;
            } else {
                cr.move_to(x, y);
                last = (x, y);
                pen = true;
                prev = Some((x, y, code));
                continue;
            }
        }
        let (dx, dy) = (x - last.0, y - last.1);
        let is_end = i + 1 == n;
        if dx * dx + dy * dy >= MIN_STEP2 || is_end {
            cr.line_to(x, y);
            last = (x, y);
        }
        prev = Some((x, y, code));
    }
    if closed && pen && !cull {
        cr.close_path();
    }
}

/// The cairo layer drawing a viewport's plots, feature dots and box.
pub struct PlotsLayer {
    pub state: Rc<PlotState>,
}

impl Layer for PlotsLayer {
    fn draw(&self, widget: &gtk::Widget, cr: &cairo::Context, view: &View) {
        let st = &self.state;
        let (antialias, theme, mono) = st.ctx.preferences().with(|p| {
            (
                p.viewport.antialias,
                p.effective_theme(),
                p.interface.mono_font.clone(),
            )
        });
        cr.set_antialias(if antialias {
            cairo::Antialias::Default
        } else {
            cairo::Antialias::None
        });
        cr.set_line_join(cairo::LineJoin::Bevel);
        cr.set_line_cap(cairo::LineCap::Round);
        let hovered = st.ctx.selection().hovered();
        let selected = st.ctx.selection().selected();
        let layers = st.layers.borrow();
        let boxes = st.boxes.borrow();
        let b = view.bounds();
        // Grow by the widest stroke so edges of thick lines are kept.
        let pad = 12.0 / view.scale.min(view.scale_y());
        let visible =
            viscas_plot::Rect::new(b.x_min - pad, b.y_min - pad, b.x_max + pad, b.y_max + pad);
        for (i, layer) in layers.iter().enumerate().filter(|(_, l)| l.error.is_none()) {
            draw_layer(
                cr,
                view,
                layer,
                boxes.get(i).map_or(&[][..], |v| &v[..]),
                &visible,
                hovered == Some(layer.node_id),
                selected.contains(&layer.node_id),
            );
        }
        cr.set_dash(&[], 0.0);

        // Feature dots and labels.
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
            set_theme(cr, theme.viewport.background);
            let _ = cr.fill_preserve();
            set_color(cr, color_of(hit.node), 1.0);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
        let hover_dot = st.hover_dot.get();
        let labelled: Vec<&FeatureHit> = pinned
            .iter()
            .chain(
                features
                    .iter()
                    .filter(|h| hover_dot.is_some_and(|p| (p - h.feature.pos).length() < 1e-12)),
            )
            .collect();
        if !labelled.is_empty() {
            let layout = widget.create_pango_layout(None);
            let family = if mono.is_empty() {
                "monospace".to_owned()
            } else {
                mono.replace(',', " ")
            };
            layout.set_font_description(Some(&pango::FontDescription::from_string(&format!(
                "{family}, monospace 8.5"
            ))));
            for hit in labelled {
                layout.set_text(&feature_label(hit));
                let (tw, th) = layout.pixel_size();
                let (tw, th) = (f64::from(tw), f64::from(th));
                let (x, y) = view.world_to_screen(hit.feature.pos.x, hit.feature.pos.y);
                let (bx, by) = (x + 8.0, y - th - 10.0);
                let (bw, bh) = (tw + 12.0, th + 4.0);
                pill(cr, bx, by, bw, bh);
                set_theme(cr, theme.ui.tooltip_bg);
                let _ = cr.fill_preserve();
                set_color(cr, color_of(hit.node), 0.9);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
                set_theme(cr, theme.ui.tooltip_fg);
                cr.move_to(bx + 6.0, by + 2.0);
                pangocairo::functions::show_layout(cr, &layout);
            }
        }

        // Box zoom.
        if let Some((a, b)) = st.zoom_box.get() {
            let (x, y) = (a.0.min(b.0), a.1.min(b.1));
            let (w, h) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
            cr.rectangle(x.floor() + 0.5, y.floor() + 0.5, w.round(), h.round());
            let (r, g, bl, _) = theme.ui.accent.rgba_f64();
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
pub fn primitive_box(p: &Primitive) -> Option<viscas_plot::Rect> {
    let pts: &[Vec2] = match p {
        Primitive::Polyline { line, .. } => &line.points,
        Primitive::Polygon(poly) => &poly.points,
        Primitive::Points(pts) => pts,
    };
    let mut it = pts.iter().filter(|q| q.is_finite());
    let first = it.next()?;
    let mut r = viscas_plot::Rect::new(first.x, first.y, first.x, first.y);
    for q in it {
        r.x_min = r.x_min.min(q.x);
        r.x_max = r.x_max.max(q.x);
        r.y_min = r.y_min.min(q.y);
        r.y_max = r.y_max.max(q.y);
    }
    Some(r)
}

/// Whether two world rectangles overlap.
pub fn overlaps(a: &viscas_plot::Rect, b: &viscas_plot::Rect) -> bool {
    a.x_min <= b.x_max && b.x_min <= a.x_max && a.y_min <= b.y_max && b.y_min <= a.y_max
}

#[allow(clippy::too_many_arguments)]
fn draw_layer(
    cr: &cairo::Context,
    view: &View,
    layer: &PlotLayer,
    boxes: &[Option<viscas_plot::Rect>],
    visible: &viscas_plot::Rect,
    hovered: bool,
    selected: bool,
) {
    let s = &layer.style;
    let base = s.line_width.clamp(0.5, 12.0);
    for (i, prim) in layer.primitives.iter().enumerate() {
        // Unknown box (stale refresh): draw it.
        if let Some(Some(b)) = boxes.get(i)
            && !overlaps(b, visible)
        {
            continue;
        }
        if boxes.get(i).is_some_and(|b| b.is_none()) {
            continue;
        }
        match prim {
            Primitive::Polygon(poly) => {
                path_polyline(cr, view, &poly.points, true, false);
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
                    path_polyline(cr, view, &line.points, line.closed, !line.closed);
                    cr.set_dash(&[], 0.0);
                    cr.set_line_width(base + 6.0);
                    set_color(cr, s.color, 0.22 * s.opacity);
                    let _ = cr.stroke();
                }
                path_polyline(cr, view, &line.points, line.closed, !line.closed);
                cr.set_dash(dash, 0.0);
                cr.set_line_width(if hovered { base + 1.75 } else { base });
                set_color(cr, s.color, s.opacity);
                let _ = cr.stroke();
            }
            Primitive::Points(pts) => {
                let r = s.point_size.clamp(1.0, 20.0) + if hovered { 1.5 } else { 0.0 };
                set_color(cr, s.color, s.opacity);
                for q in pts.iter().filter(|q| q.is_finite()) {
                    let (x, y) = view.world_to_screen(q.x, q.y);
                    cr.new_sub_path();
                    cr.arc(clamp_coord(x), clamp_coord(y), r, 0.0, TAU);
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
            nearest_curve(&layers, &view, (400.0, 305.0), HOVER_PX),
            Some(NodeId(1))
        );
        // 4 px below y = 1 (screen y 240).
        assert_eq!(
            nearest_curve(&layers, &view, (420.0, 244.0), HOVER_PX),
            Some(NodeId(2))
        );
        assert_eq!(
            nearest_curve(&layers, &view, (580.0, 422.0), HOVER_PX),
            Some(NodeId(3))
        );
        assert_eq!(
            nearest_curve(&layers, &view, (400.0, 270.0), HOVER_PX),
            None
        );
        let mut broken = layers.clone();
        broken[0].error = Some("boom".into());
        assert_eq!(
            nearest_curve(&broken, &view, (400.0, 305.0), HOVER_PX),
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
}
