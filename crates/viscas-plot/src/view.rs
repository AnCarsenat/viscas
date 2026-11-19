//! Viewport: world bounds, pixel size, transforms, pan/zoom and grid ticks.

use crate::geom::{Rect, Vec2};

/// Smallest allowed span relative to the magnitude of the view center.
const MIN_REL_SPAN: f64 = 1e-11;
/// Smallest allowed absolute span.
const MIN_ABS_SPAN: f64 = 1e-300;
/// Largest allowed span.
const MAX_SPAN: f64 = 1e15;

/// Maps world coordinates (y up) to screen pixels (y down).
///
/// Invariant: spans are finite and positive, pixel sizes are at least 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
    width_px: f64,
    height_px: f64,
}

/// Affine world→screen map: `sx_screen = sx * x + tx`, `sy_screen = sy * y + ty`.
///
/// Matches a Cairo matrix `(xx=sx, yx=0, xy=0, yy=sy, x0=tx, y0=ty)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub sx: f64,
    pub sy: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Affine {
    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(self.sx * p.x + self.tx, self.sy * p.y + self.ty)
    }

    pub fn inverse(&self) -> Affine {
        Affine {
            sx: 1.0 / self.sx,
            sy: 1.0 / self.sy,
            tx: -self.tx / self.sx,
            ty: -self.ty / self.sy,
        }
    }
}

impl Default for View {
    /// `[-10, 10]` horizontally on a 1200×800 canvas with locked aspect.
    fn default() -> Self {
        Self::centered(Vec2::new(0.0, 0.0), 20.0 / 1200.0, 1200.0, 800.0)
    }
}

fn span_ok(lo: f64, hi: f64) -> bool {
    let span = hi - lo;
    let c = 0.5 * (lo + hi);
    lo.is_finite()
        && hi.is_finite()
        && span.is_finite()
        && span <= MAX_SPAN
        && span >= (c.abs() * MIN_REL_SPAN).max(MIN_ABS_SPAN)
}

/// Why a [`View`] could not be constructed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// Bounds are non-finite, inverted, or the span is too small/large for
    /// `f64` sampling (relative span below 1e-11 or above 1e15).
    InvalidBounds,
    /// Pixel size is non-finite or below 1.
    InvalidSize,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ViewError::InvalidBounds => "invalid view bounds",
            ViewError::InvalidSize => "invalid view pixel size",
        })
    }
}

impl std::error::Error for ViewError {}

fn size_ok(v: f64) -> bool {
    v.is_finite() && v >= 1.0
}

impl View {
    /// Create a view showing `bounds` on a `width_px`×`height_px` canvas.
    pub fn try_new(bounds: Rect, width_px: f64, height_px: f64) -> Result<Self, ViewError> {
        if !(size_ok(width_px) && size_ok(height_px)) {
            return Err(ViewError::InvalidSize);
        }
        if !(span_ok(bounds.x_min, bounds.x_max) && span_ok(bounds.y_min, bounds.y_max)) {
            return Err(ViewError::InvalidBounds);
        }
        Ok(Self {
            x_min: bounds.x_min,
            x_max: bounds.x_max,
            y_min: bounds.y_min,
            y_max: bounds.y_max,
            width_px,
            height_px,
        })
    }

    /// Like [`View::try_new`] for inputs known to be valid (constants, tests).
    ///
    /// # Panics
    /// If the bounds or size are invalid; use [`View::try_new`] for user input
    /// and [`View::default`] as a fallback.
    pub fn new(bounds: Rect, width_px: f64, height_px: f64) -> Self {
        match Self::try_new(bounds, width_px, height_px) {
            Ok(v) => v,
            Err(e) => panic!("{e}: {bounds:?} at {width_px}x{height_px} px"),
        }
    }

    /// Aspect-locked view centered at `center` with `units_per_px` world units
    /// per pixel on both axes.
    pub fn try_centered(
        center: Vec2,
        units_per_px: f64,
        width_px: f64,
        height_px: f64,
    ) -> Result<Self, ViewError> {
        if !(size_ok(width_px) && size_ok(height_px)) {
            return Err(ViewError::InvalidSize);
        }
        let hx = 0.5 * units_per_px * width_px;
        let hy = 0.5 * units_per_px * height_px;
        Self::try_new(
            Rect::new(center.x - hx, center.y - hy, center.x + hx, center.y + hy),
            width_px,
            height_px,
        )
    }

    /// # Panics
    /// If the resulting view is invalid (see [`View::try_centered`]).
    pub fn centered(center: Vec2, units_per_px: f64, width_px: f64, height_px: f64) -> Self {
        match Self::try_centered(center, units_per_px, width_px, height_px) {
            Ok(v) => v,
            Err(e) => panic!("{e}"),
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(self.x_min, self.y_min, self.x_max, self.y_max)
    }

    pub fn width_px(&self) -> f64 {
        self.width_px
    }

    pub fn height_px(&self) -> f64 {
        self.height_px
    }

    /// World units covered by one horizontal pixel.
    pub fn units_per_px_x(&self) -> f64 {
        (self.x_max - self.x_min) / self.width_px
    }

    /// World units covered by one vertical pixel.
    pub fn units_per_px_y(&self) -> f64 {
        (self.y_max - self.y_min) / self.height_px
    }

    /// Ratio of vertical to horizontal world units per pixel (1 = locked).
    pub fn aspect_ratio(&self) -> f64 {
        self.units_per_px_y() / self.units_per_px_x()
    }

    pub fn is_aspect_locked(&self) -> bool {
        (self.aspect_ratio() - 1.0).abs() < 1e-9
    }

    /// The world→screen transform for the UI.
    pub fn transform(&self) -> Affine {
        let sx = self.width_px / (self.x_max - self.x_min);
        let sy = -self.height_px / (self.y_max - self.y_min);
        Affine {
            sx,
            sy,
            tx: -self.x_min * sx,
            ty: -self.y_max * sy,
        }
    }

    pub fn world_to_screen(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            (p.x - self.x_min) / (self.x_max - self.x_min) * self.width_px,
            (self.y_max - p.y) / (self.y_max - self.y_min) * self.height_px,
        )
    }

    pub fn screen_to_world(&self, s: Vec2) -> Vec2 {
        Vec2::new(
            self.x_min + s.x / self.width_px * (self.x_max - self.x_min),
            self.y_max - s.y / self.height_px * (self.y_max - self.y_min),
        )
    }

    /// Pan by a screen-space drag of `(dx, dy)` pixels: content follows the
    /// pointer.
    pub fn pan_px(&mut self, dx: f64, dy: f64) {
        let wx = -dx * self.units_per_px_x();
        let wy = dy * self.units_per_px_y();
        self.pan_world(wx, wy);
    }

    /// Shift the visible window by `(dx, dy)` world units.
    pub fn pan_world(&mut self, dx: f64, dy: f64) {
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        let (a, b) = (self.x_min + dx, self.x_max + dx);
        let (c, d) = (self.y_min + dy, self.y_max + dy);
        if span_ok(a, b) && span_ok(c, d) {
            (self.x_min, self.x_max, self.y_min, self.y_max) = (a, b, c, d);
        }
    }

    /// Zoom about a world point; `factor > 1` zooms in. The point stays fixed
    /// on screen. Requests that would make the view degenerate are ignored.
    pub fn zoom_about_world(&mut self, p: Vec2, factor: f64) {
        self.zoom_xy_about_world(p, factor, factor);
    }

    /// Zoom about a screen point (e.g. the mouse cursor).
    pub fn zoom_about_screen(&mut self, s: Vec2, factor: f64) {
        let p = self.screen_to_world(s);
        self.zoom_about_world(p, factor);
    }

    /// Zoom with independent factors per axis (axis-drag zoom).
    pub fn zoom_xy_about_world(&mut self, p: Vec2, fx: f64, fy: f64) {
        if !(fx.is_finite() && fy.is_finite() && fx > 0.0 && fy > 0.0 && p.is_finite()) {
            return;
        }
        let a = p.x + (self.x_min - p.x) / fx;
        let b = p.x + (self.x_max - p.x) / fx;
        let c = p.y + (self.y_min - p.y) / fy;
        let d = p.y + (self.y_max - p.y) / fy;
        if span_ok(a, b) && span_ok(c, d) {
            (self.x_min, self.x_max, self.y_min, self.y_max) = (a, b, c, d);
        }
    }

    /// Make one world unit the same number of pixels on both axes, keeping the
    /// center and the horizontal scale.
    pub fn lock_aspect(&mut self) {
        let c = self.bounds().center();
        let hy = 0.5 * self.units_per_px_x() * self.height_px;
        if span_ok(c.y - hy, c.y + hy) {
            self.y_min = c.y - hy;
            self.y_max = c.y + hy;
        }
    }

    /// Change the canvas size, keeping the center and units per pixel (so
    /// content does not stretch when the window is resized). Sizes below one
    /// pixel (a collapsed widget) are treated as one pixel; non-finite sizes
    /// or sizes that would overflow the span leave the view unchanged.
    pub fn resize(&mut self, width_px: f64, height_px: f64) {
        if !(width_px.is_finite() && height_px.is_finite()) {
            return;
        }
        let (ux, uy) = (self.units_per_px_x(), self.units_per_px_y());
        let c = self.bounds().center();
        let (w, h) = (width_px.max(1.0), height_px.max(1.0));
        let (hx, hy) = (0.5 * ux * w, 0.5 * uy * h);
        if let Ok(v) = Self::try_new(Rect::new(c.x - hx, c.y - hy, c.x + hx, c.y + hy), w, h) {
            *self = v;
        }
    }

    /// Grid ticks along x with majors at least `min_major_px` pixels apart.
    pub fn x_ticks(&self, min_major_px: f64) -> Ticks {
        Ticks::for_range(
            self.x_min,
            self.x_max,
            self.units_per_px_x() * min_major_px.max(1.0),
        )
    }

    /// Grid ticks along y with majors at least `min_major_px` pixels apart.
    pub fn y_ticks(&self, min_major_px: f64) -> Ticks {
        Ticks::for_range(
            self.y_min,
            self.y_max,
            self.units_per_px_y() * min_major_px.max(1.0),
        )
    }
}

/// Smallest "nice" step `m × 10^k` with `m ∈ {1, 2, 5}` that is `>= raw`.
pub fn nice_step(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let k = raw.log10().floor();
    let base = 10f64.powf(k);
    for m in [1.0, 2.0, 5.0, 10.0] {
        let s = m * base;
        // Small slack against log10/powf rounding.
        if s >= raw * (1.0 - 1e-12) {
            return s;
        }
    }
    10.0 * base
}

/// Minor step for a nice major step: 1→0.2, 2→0.5, 5→1 (×10^k).
pub fn minor_step(major: f64) -> f64 {
    let k = major.log10().floor();
    let m = (major / 10f64.powf(k)).round();
    if m == 2.0 { major / 4.0 } else { major / 5.0 }
}

/// Grid line positions for one axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticks {
    pub major_step: f64,
    pub minor_step: f64,
    /// Major positions inside the range.
    pub major: Vec<f64>,
    /// Minor positions inside the range that are not major positions.
    pub minor: Vec<f64>,
}

/// Safety cap on the number of generated lines per axis.
const MAX_TICKS: f64 = 10_000.0;

impl Ticks {
    /// Ticks for `[lo, hi]` with major step at least `min_step` world units.
    pub fn for_range(lo: f64, hi: f64, min_step: f64) -> Self {
        let major_step = nice_step(min_step);
        let minor_step = minor_step(major_step);
        if !(major_step.is_finite() && minor_step.is_finite() && minor_step > 0.0) {
            return Self {
                major_step,
                minor_step,
                major: Vec::new(),
                minor: Vec::new(),
            };
        }
        let ratio = ((major_step / minor_step).round() as i64).max(1);
        let major = Self::positions(lo, hi, major_step, None);
        let minor = Self::positions(lo, hi, minor_step, Some(ratio));
        Self {
            major_step,
            minor_step,
            major,
            minor,
        }
    }

    fn positions(lo: f64, hi: f64, step: f64, skip_every: Option<i64>) -> Vec<f64> {
        let (k0, k1) = ((lo / step).ceil(), (hi / step).floor());
        if !(k0.is_finite() && k1.is_finite()) || k1 - k0 > MAX_TICKS {
            return Vec::new();
        }
        let (k0, k1) = (k0 as i64, k1 as i64);
        (k0..=k1)
            .filter(|k| skip_every.is_none_or(|r| k.rem_euclid(r) != 0))
            .map(|k| {
                let v = k as f64 * step;
                if v.abs() < step * 1e-9 { 0.0 } else { v }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs()))
    }

    fn view() -> View {
        View::new(Rect::new(-10.0, -5.0, 10.0, 5.0), 1000.0, 500.0)
    }

    #[test]
    fn default_is_aspect_locked() {
        let v = View::default();
        assert!(v.is_aspect_locked());
        assert!(close(v.bounds().x_min, -10.0));
    }

    #[test]
    fn world_screen_round_trip() {
        let v = view();
        let p = Vec2::new(3.25, -1.5);
        let s = v.world_to_screen(p);
        let q = v.screen_to_world(s);
        assert!(close(p.x, q.x) && close(p.y, q.y));
        assert_eq!(
            v.world_to_screen(Vec2::new(-10.0, 5.0)),
            Vec2::new(0.0, 0.0)
        );
        assert_eq!(
            v.world_to_screen(Vec2::new(10.0, -5.0)),
            Vec2::new(1000.0, 500.0)
        );
    }

    #[test]
    fn affine_matches_world_to_screen() {
        let v = view();
        let t = v.transform();
        for p in [
            Vec2::new(0.0, 0.0),
            Vec2::new(7.0, -3.0),
            Vec2::new(-9.5, 4.0),
        ] {
            let a = t.apply(p);
            let b = v.world_to_screen(p);
            assert!(close(a.x, b.x) && close(a.y, b.y));
            let back = t.inverse().apply(a);
            assert!(close(back.x, p.x) && close(back.y, p.y));
        }
    }

    #[test]
    fn pan_px_moves_content_with_pointer() {
        let mut v = view();
        let p = Vec2::new(1.0, 1.0);
        let before = v.world_to_screen(p);
        v.pan_px(50.0, -20.0);
        let after = v.world_to_screen(p);
        assert!(close(after.x - before.x, 50.0));
        assert!(close(after.y - before.y, -20.0));
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut v = view();
        let s = Vec2::new(250.0, 100.0);
        let w = v.screen_to_world(s);
        v.zoom_about_screen(s, 2.0);
        let s2 = v.world_to_screen(w);
        assert!(close(s.x, s2.x) && close(s.y, s2.y));
        assert!(close(v.bounds().width(), 10.0));
        v.zoom_about_world(w, 0.5);
        assert!(close(v.bounds().width(), 20.0));
    }

    #[test]
    fn zoom_rejects_degenerate() {
        let mut v = view();
        let before = v;
        v.zoom_about_world(Vec2::new(3.0, 1.0), 1e30);
        assert_eq!(v, before);
        v.zoom_about_world(Vec2::new(0.0, 0.0), f64::NAN);
        assert_eq!(v, before);
        v.zoom_about_world(Vec2::new(0.0, 0.0), -2.0);
        assert_eq!(v, before);
    }

    #[test]
    fn zoom_xy_independent() {
        let mut v = view();
        v.zoom_xy_about_world(Vec2::new(0.0, 0.0), 2.0, 1.0);
        assert!(close(v.bounds().width(), 10.0));
        assert!(close(v.bounds().height(), 10.0));
    }

    #[test]
    fn lock_aspect_adjusts_y() {
        let mut v = View::new(Rect::new(0.0, 0.0, 10.0, 1.0), 100.0, 100.0);
        assert!(!v.is_aspect_locked());
        v.lock_aspect();
        assert!(v.is_aspect_locked());
        assert!(close(v.bounds().height(), 10.0));
        assert!(close(v.bounds().center().y, 0.5));
    }

    #[test]
    fn resize_keeps_scale_and_center() {
        let mut v = view();
        let (ux, uy) = (v.units_per_px_x(), v.units_per_px_y());
        v.resize(2000.0, 300.0);
        assert!(close(v.units_per_px_x(), ux) && close(v.units_per_px_y(), uy));
        assert!(close(v.bounds().center().x, 0.0));
        assert!(close(v.bounds().width(), 40.0));
    }

    #[test]
    fn invalid_views_are_rejected() {
        let ok = Rect::new(-1.0, -1.0, 1.0, 1.0);
        assert_eq!(
            View::try_new(Rect::new(1.0, 0.0, 1.0, 2.0), 10.0, 10.0),
            Err(ViewError::InvalidBounds)
        );
        assert_eq!(
            View::try_new(Rect::new(0.0, f64::NAN, 1.0, 2.0), 10.0, 10.0),
            Err(ViewError::InvalidBounds)
        );
        assert_eq!(
            View::try_new(Rect::new(2.0, 0.0, 1.0, 1.0), 10.0, 10.0),
            Err(ViewError::InvalidBounds)
        );
        assert_eq!(View::try_new(ok, 0.0, 10.0), Err(ViewError::InvalidSize));
        assert_eq!(
            View::try_new(ok, 10.0, f64::NAN),
            Err(ViewError::InvalidSize)
        );
        assert!(View::try_new(ok, 10.0, 10.0).is_ok());
        assert!(View::try_centered(Vec2::new(0.0, 0.0), f64::INFINITY, 10.0, 10.0).is_err());
    }

    #[test]
    #[should_panic(expected = "invalid view bounds")]
    fn new_panics_on_invalid_bounds() {
        let _ = View::new(Rect::new(1.0, 0.0, 1.0, 1.0), 10.0, 10.0);
    }

    #[test]
    fn resize_ignores_garbage_and_clamps_collapsed() {
        let mut v = view();
        let before = v;
        v.resize(f64::NAN, 10.0);
        assert_eq!(v, before);
        v.resize(0.0, 0.0);
        assert_eq!(v.width_px(), 1.0);
    }

    #[test]
    fn ticks_with_overflowing_steps_do_not_panic() {
        // nice_step overflows to +inf here (2e308); this used to panic.
        let t = Ticks::for_range(0.0, 1e308, 1.5e308);
        assert!(t.major_step.is_infinite());
        assert!(t.major.is_empty() && t.minor.is_empty());
        let t = Ticks::for_range(-1.0, 1.0, f64::MAX);
        assert!(t.minor.is_empty());
        let t = Ticks::for_range(0.0, 1e-300, 1e-310);
        assert!(t.major.len() <= 10_001);
    }

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(0.7), 1.0);
        assert_eq!(nice_step(1.0), 1.0);
        assert_eq!(nice_step(1.3), 2.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(7.0), 10.0);
        assert!(close(nice_step(0.023), 0.05));
        assert!(close(nice_step(180.0), 200.0));
        assert!(close(nice_step(0.1), 0.1));
        assert_eq!(nice_step(-1.0), 1.0);
    }

    #[test]
    fn minor_steps() {
        assert!(close(minor_step(1.0), 0.2));
        assert!(close(minor_step(2.0), 0.5));
        assert!(close(minor_step(5.0), 1.0));
        assert!(close(minor_step(0.02), 0.005));
    }

    #[test]
    fn ticks_cover_range() {
        let v = view(); // 20 units over 1000 px → 0.02 units/px
        let t = v.x_ticks(80.0); // ≥ 1.6 units → step 2
        assert_eq!(t.major_step, 2.0);
        assert_eq!(t.major.first(), Some(&-10.0));
        assert_eq!(t.major.last(), Some(&10.0));
        assert_eq!(t.major.len(), 11);
        assert!(t.major.contains(&0.0));
        // minors at 0.5 that are not majors: 4 per gap minus the major itself
        assert_eq!(t.minor.len(), 10 * 3);
        assert!(t.minor.iter().all(|m| (m / 2.0).fract().abs() > 1e-9));
    }

    #[test]
    fn ticks_exact_zero_and_cap() {
        let t = Ticks::for_range(-0.3, 0.3, 0.1);
        assert!(t.major.contains(&0.0));
        assert!(t.major.iter().any(|&x| close(x, 0.1)));
        let huge = Ticks::for_range(0.0, 1e9, 1.0);
        assert!(huge.major.is_empty());
    }
}
