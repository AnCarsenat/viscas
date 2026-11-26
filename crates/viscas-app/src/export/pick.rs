//! Viewport interactions of the export dialog: two handles dragged along
//! a curve to pick a part of it, and the Shift+E rubber band box.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::cairo;
use viscas_engine::PlotShape;
use viscas_engine::export::{nearest_param, point_at};
use viscas_plot::Vec2;
use viscas_scene::Color;

use crate::viewport::Layer;
use crate::viewport::view::View;

/// Handle grab distance in pixels.
const GRAB_PX: f64 = 12.0;
/// Samples of the highlighted part.
const HIGHLIGHT_SAMPLES: usize = 400;
/// Probes of the nearest-parameter search.
const SEARCH_SAMPLES: usize = 2000;

/// Callback receiving the picked parameter range.
pub type OnChange = Rc<dyn Fn(f64, f64)>;

/// An active pick: a curve and two parameter handles.
pub struct Pick {
    pub shape: PlotShape,
    pub color: Color,
    /// Parameter interval searched when dragging (`None` = the visible
    /// x or y span, for explicit curves).
    pub domain: Option<(f64, f64)>,
    pub t: [f64; 2],
    pub on_change: OnChange,
}

thread_local! {
    static PICK: RefCell<Option<Pick>> = const { RefCell::new(None) };
    static DRAGGING: Cell<Option<usize>> = const { Cell::new(None) };
    static BOX_ARMED: Cell<bool> = const { Cell::new(false) };
}

fn redraw() {
    if let Some(vp) = crate::viewport::active() {
        vp.queue_draw();
    }
}

/// Starts picking (replacing any previous pick).
pub fn start(pick: Pick) {
    PICK.with_borrow_mut(|p| *p = Some(pick));
    redraw();
}

/// Ends picking.
pub fn stop() {
    PICK.with_borrow_mut(|p| *p = None);
    DRAGGING.set(None);
    redraw();
}

pub fn is_active() -> bool {
    PICK.with_borrow(Option::is_some)
}

/// Moves the handles (e.g. after the range fields changed).
pub fn set_range(t0: f64, t1: f64) {
    PICK.with_borrow_mut(|p| {
        if let Some(p) = p {
            p.t = [t0, t1];
        }
    });
    redraw();
}

/// The next left drag in a viewport draws an export box.
pub fn arm_box() {
    BOX_ARMED.set(true);
    if let Some(vp) = crate::viewport::active() {
        vp.grab_focus();
    }
    super::show_status(
        "drag a box in the viewport to export it (Esc cancels)",
        None,
        false,
    );
}

/// Disarms the box mode (Escape).
pub fn disarm_box() {
    BOX_ARMED.set(false);
}

/// Consumes the armed box mode (viewport drag begin).
pub fn take_box() -> bool {
    BOX_ARMED.replace(false)
}

fn domain_of(p: &Pick, view: &View) -> (f64, f64) {
    p.domain.unwrap_or_else(|| {
        let b = view.bounds();
        match p.shape {
            PlotShape::ExplicitX(_) => (b.y_min, b.y_max),
            _ => (b.x_min, b.x_max),
        }
    })
}

fn screen_of(p: &Pick, view: &View, t: f64) -> Option<(f64, f64)> {
    point_at(&p.shape, t)
        .filter(|q| q.is_finite())
        .map(|q| view.world_to_screen(q.x, q.y))
}

/// Viewport drag begin: grabs a handle near `(sx, sy)` (or the nearer
/// handle when the press is on the curve). `true` when the pick takes it.
pub fn press(view: &View, sx: f64, sy: f64) -> bool {
    let grabbed = PICK.with_borrow(|p| {
        let p = p.as_ref()?;
        let dist =
            |t: f64| screen_of(p, view, t).map_or(f64::INFINITY, |(x, y)| (x - sx).hypot(y - sy));
        let (d0, d1) = (dist(p.t[0]), dist(p.t[1]));
        let i = if d0 <= d1 { 0 } else { 1 };
        (d0.min(d1) <= GRAB_PX).then_some(i)
    });
    match grabbed {
        Some(i) => {
            DRAGGING.set(Some(i));
            true
        }
        None => false,
    }
}

/// Viewport drag update: moves the grabbed handle to the curve point
/// nearest the pointer.
pub fn drag_to(view: &View, sx: f64, sy: f64) {
    let Some(i) = DRAGGING.get() else {
        return;
    };
    let changed = PICK.with_borrow_mut(|p| {
        let p = p.as_mut()?;
        let (wx, wy) = view.screen_to_world(sx, sy);
        let scale = (1.0 / view.scale, 1.0 / view.scale_y());
        let t = nearest_param(
            &p.shape,
            domain_of(p, view),
            Vec2::new(wx, wy),
            scale,
            SEARCH_SAMPLES,
        )?;
        p.t[i] = t;
        Some((p.t, p.on_change.clone()))
    });
    if let Some((t, f)) = changed {
        f(t[0].min(t[1]), t[0].max(t[1]));
        redraw();
    }
}

/// Viewport drag end.
pub fn release() -> bool {
    DRAGGING.take().is_some()
}

/// Is a handle being dragged?
pub fn dragging() -> bool {
    DRAGGING.get().is_some()
}

/// Draws the picked part and its handles on top of the plots.
pub struct PickLayer;

impl Layer for PickLayer {
    fn draw(&self, _widget: &gtk::Widget, cr: &cairo::Context, view: &View) {
        PICK.with_borrow(|p| {
            if let Some(p) = p {
                draw_pick(cr, view, p);
            }
        });
    }
}

fn set(cr: &cairo::Context, c: Color, a: f64) {
    cr.set_source_rgba(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        a,
    );
}

fn draw_pick(cr: &cairo::Context, view: &View, p: &Pick) {
    let (t0, t1) = (p.t[0].min(p.t[1]), p.t[0].max(p.t[1]));
    let mut last: Option<(f64, f64)> = None;
    let jump = view.size.0.max(view.size.1);
    for i in 0..=HIGHLIGHT_SAMPLES {
        let t = t0 + (t1 - t0) * i as f64 / HIGHLIGHT_SAMPLES as f64;
        match screen_of(p, view, t) {
            Some((x, y)) if x.abs() < 1e6 && y.abs() < 1e6 => {
                match last {
                    Some((lx, ly)) if (x - lx).hypot(y - ly) < jump => cr.line_to(x, y),
                    _ => cr.move_to(x, y),
                }
                last = Some((x, y));
            }
            _ => last = None,
        }
    }
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_width(7.0);
    set(cr, p.color, 0.35);
    let _ = cr.stroke();
    for (k, t) in p.t.iter().enumerate() {
        let Some((x, y)) = screen_of(p, view, *t) else {
            continue;
        };
        cr.new_sub_path();
        cr.arc(
            x,
            y,
            if DRAGGING.get() == Some(k) { 7.5 } else { 6.0 },
            0.0,
            TAU,
        );
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        let _ = cr.fill_preserve();
        set(cr, p.color, 1.0);
        cr.set_line_width(2.5);
        let _ = cr.stroke();
    }
}
