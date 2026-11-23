//! Pure 2D view math: world <-> screen transforms, pan/zoom and grid ticks.
//!
//! Nothing in here depends on GTK so it can be unit tested in isolation.
//! Screen coordinates are in logical pixels with the origin at the top-left
//! corner and `y` pointing down; world coordinates use the usual math
//! orientation with `y` pointing up.

/// Smallest allowed zoom level, in pixels per world unit.
pub const MIN_SCALE: f64 = 1e-9;
/// Largest allowed zoom level, in pixels per world unit.
pub const MAX_SCALE: f64 = 1e6;
/// Largest allowed distance of the view center from the origin, per axis.
///
/// Together with [`MAX_SCALE`] this keeps tick indices far below 2^53 so
/// `f64` grid math stays exact.
pub const MAX_CENTER: f64 = 1e9;

/// A 2D camera looking at the Cartesian plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// World coordinates shown at the center of the screen.
    pub center: (f64, f64),
    /// Zoom level of the x axis, in pixels per world unit.
    pub scale: f64,
    /// Vertical over horizontal scale (1 when the aspect is locked).
    pub aspect: f64,
    /// Screen size in logical pixels.
    pub size: (f64, f64),
}

impl Default for View {
    fn default() -> Self {
        Self {
            center: (0.0, 0.0),
            scale: 60.0,
            aspect: 1.0,
            size: (800.0, 600.0),
        }
    }
}

/// Axis-aligned rectangle in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
}

impl View {
    /// Zoom level of the y axis, in pixels per world unit.
    pub fn scale_y(&self) -> f64 {
        self.scale * self.aspect
    }

    /// Converts a world point to screen coordinates.
    pub fn world_to_screen(&self, x: f64, y: f64) -> (f64, f64) {
        let (w, h) = self.size;
        (
            w / 2.0 + (x - self.center.0) * self.scale,
            h / 2.0 - (y - self.center.1) * self.scale_y(),
        )
    }

    /// Converts a screen point to world coordinates.
    pub fn screen_to_world(&self, sx: f64, sy: f64) -> (f64, f64) {
        let (w, h) = self.size;
        (
            self.center.0 + (sx - w / 2.0) / self.scale,
            self.center.1 - (sy - h / 2.0) / self.scale_y(),
        )
    }

    /// Equal scales on both axes (keeps the x scale).
    pub fn lock_aspect(&mut self) {
        self.aspect = 1.0;
    }

    /// Shows the world rectangle spanned by two screen points. With
    /// `locked` the aspect stays 1 and the whole rectangle fits; otherwise
    /// each axis is fitted on its own. Rectangles smaller than 4 px in
    /// either direction are ignored (returns `false`).
    pub fn fit_screen_rect(&mut self, a: (f64, f64), b: (f64, f64), locked: bool) -> bool {
        let (w, h) = self.size;
        let (dw, dh) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
        if dw < 4.0 || dh < 4.0 || w <= 0.0 || h <= 0.0 {
            return false;
        }
        let p = self.screen_to_world(a.0, a.1);
        let q = self.screen_to_world(b.0, b.1);
        let (bw, bh) = ((p.0 - q.0).abs(), (p.1 - q.1).abs());
        let sx = w / bw;
        let sy = h / bh;
        if locked {
            self.scale = sx.min(sy).clamp(MIN_SCALE, MAX_SCALE);
            self.aspect = 1.0;
        } else {
            self.scale = sx.clamp(MIN_SCALE, MAX_SCALE);
            self.aspect = (sy / self.scale).clamp(1e-6, 1e6);
        }
        self.center = ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
        self.clamp_center();
        true
    }

    /// The sampling view for the plot engine: the visible rectangle grown
    /// by `margin` (fraction of its size) on every side, with the pixel
    /// size grown alike so the resolution matches the screen.
    pub fn plot_view(&self, margin: f64) -> Option<viscas_plot::View> {
        let (w, h) = self.size;
        if w < 1.0 || h < 1.0 {
            return None;
        }
        let b = self.bounds();
        let (mx, my) = ((b.x_max - b.x_min) * margin, (b.y_max - b.y_min) * margin);
        let rect = viscas_plot::Rect::new(b.x_min - mx, b.y_min - my, b.x_max + mx, b.y_max + my);
        let k = 1.0 + 2.0 * margin;
        viscas_plot::View::try_new(rect, (w * k).round(), (h * k).round()).ok()
    }

    /// Whether the visible rectangle lies inside `plot` and was sampled
    /// at (nearly) this resolution, so cached layers are still sharp.
    pub fn covered_by(&self, plot: &viscas_plot::View) -> bool {
        let (w, _) = self.size;
        let b = self.bounds();
        let r = plot.bounds();
        let ratio = (plot.units_per_px_x() * self.scale).abs();
        let ratio_y = (plot.units_per_px_y() * self.scale_y()).abs();
        w > 0.0
            && (ratio - 1.0).abs() < 0.01
            && (ratio_y - 1.0).abs() < 0.01
            && b.x_min >= r.x_min
            && b.x_max <= r.x_max
            && b.y_min >= r.y_min
            && b.y_max <= r.y_max
    }

    /// The world rectangle currently visible on screen.
    pub fn bounds(&self) -> Bounds {
        let (x_min, y_max) = self.screen_to_world(0.0, 0.0);
        let (x_max, y_min) = self.screen_to_world(self.size.0, self.size.1);
        Bounds {
            x_min,
            x_max,
            y_min,
            y_max,
        }
    }

    /// Moves the view so that content follows a pointer drag of `(dx, dy)` pixels.
    pub fn pan_by_pixels(&mut self, dx: f64, dy: f64) {
        self.center.0 -= dx / self.scale;
        self.center.1 += dy / self.scale_y();
        self.clamp_center();
    }

    /// Multiplies the zoom by `factor`, keeping the world point under the
    /// screen position `(sx, sy)` fixed.
    pub fn zoom_at(&mut self, factor: f64, sx: f64, sy: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let anchor = self.screen_to_world(sx, sy);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let moved = self.screen_to_world(sx, sy);
        self.center.0 += anchor.0 - moved.0;
        self.center.1 += anchor.1 - moved.1;
        self.clamp_center();
    }

    fn clamp_center(&mut self) {
        let clamp = |c: f64| {
            if c.is_finite() {
                c.clamp(-MAX_CENTER, MAX_CENTER)
            } else {
                0.0
            }
        };
        self.center = (clamp(self.center.0), clamp(self.center.1));
    }
}

/// Smallest "nice" number (1, 2 or 5 times a power of ten) that is `>= raw`.
///
/// Returns `1.0` for non-positive or non-finite input.
pub fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    // Small tolerance so that exact powers like 0.2 are not bumped to 0.5
    // because of floating point noise.
    let mantissa = raw / base;
    let nice = if mantissa <= 1.0 + 1e-9 {
        1.0
    } else if mantissa <= 2.0 + 1e-9 {
        2.0
    } else if mantissa <= 5.0 + 1e-9 {
        5.0
    } else {
        10.0
    };
    nice * base
}

/// Major and minor grid spacing in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSteps {
    pub major: f64,
    pub minor: f64,
}

/// Picks grid spacing so major lines are at least `min_major_px` pixels apart.
///
/// Minor lines subdivide a major step into 4 parts for `2×10^k` steps and
/// 5 parts otherwise, so minor lines always land on round values.
pub fn grid_steps(scale: f64, min_major_px: f64) -> GridSteps {
    let major = nice_step(min_major_px / scale);
    let mantissa = major / 10f64.powf(major.log10().floor());
    let divisions = if (mantissa - 2.0).abs() < 1e-6 {
        4.0
    } else {
        5.0
    };
    GridSteps {
        major,
        minor: major / divisions,
    }
}

/// Multiples of `step` within `[min, max]`, as `(index, value)` pairs.
///
/// The index is `value / step` and is handy to tell major from minor lines.
/// Returns nothing if the range would produce an absurd number of lines or
/// tick indices too large to be represented exactly (>= 2^53).
pub fn ticks(min: f64, max: f64, step: f64) -> impl Iterator<Item = (i64, f64)> {
    const MAX_TICKS: f64 = 10_000.0;
    const MAX_INDEX: f64 = 9_007_199_254_740_992.0; // 2^53
    let valid = step.is_finite()
        && step > 0.0
        && min.is_finite()
        && max.is_finite()
        && (min / step).abs() < MAX_INDEX
        && (max / step).abs() < MAX_INDEX;
    let (first, last) = if valid && (max - min) / step <= MAX_TICKS {
        ((min / step).ceil() as i64, (max / step).floor() as i64)
    } else {
        (1, 0)
    };
    (first..=last).map(move |i| (i, i as f64 * step))
}

/// Formats a tick value with just enough decimals for the given step.
pub fn format_tick(value: f64, step: f64) -> String {
    if value == 0.0 || value.abs() < step * 1e-6 {
        return "0".to_owned();
    }
    let magnitude = step.abs().log10().floor();
    if !(-4.0..6.0).contains(&magnitude) {
        let s = format!("{value:e}");
        return trim_exponent(&s);
    }
    let decimals = if magnitude < 0.0 {
        (-magnitude) as usize
    } else {
        0
    };
    format!("{value:.decimals$}").replace('-', "\u{2212}")
}

/// Decimals needed to show world coordinates at one-pixel precision.
pub fn label_digits(scale: f64) -> usize {
    if !scale.is_finite() || scale <= 0.0 {
        return 3;
    }
    scale.log10().ceil().clamp(0.0, 9.0) as usize
}

/// Shortens Rust's `{:e}` output (e.g. `1.0000000000000002e-5`) to a clean label.
fn trim_exponent(s: &str) -> String {
    let (mantissa, exp) = s.split_once('e').unwrap_or((s, "0"));
    let mantissa: f64 = mantissa.parse().unwrap_or(0.0);
    let mut m = format!("{mantissa:.3}");
    while m.ends_with('0') {
        m.pop();
    }
    if m.ends_with('.') {
        m.pop();
    }
    format!("{m}e{exp}").replace('-', "\u{2212}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn origin_maps_to_screen_center() {
        let v = View::default();
        assert_eq!(v.world_to_screen(0.0, 0.0), (400.0, 300.0));
    }

    #[test]
    fn y_axis_points_up() {
        let v = View::default();
        let (_, sy) = v.world_to_screen(0.0, 1.0);
        assert!(sy < 300.0);
    }

    #[test]
    fn transforms_round_trip() {
        let v = View {
            center: (3.5, -2.25),
            scale: 17.0,
            aspect: 1.0,
            size: (640.0, 480.0),
        };
        for &(x, y) in &[(0.0, 0.0), (1.0, -1.0), (123.4, 56.7), (-9.0, 1e3)] {
            let (sx, sy) = v.world_to_screen(x, y);
            let (wx, wy) = v.screen_to_world(sx, sy);
            assert!(approx(x, wx) && approx(y, wy), "{x},{y} -> {wx},{wy}");
        }
    }

    #[test]
    fn bounds_match_screen_corners() {
        let v = View {
            center: (1.0, 2.0),
            scale: 100.0,
            aspect: 1.0,
            size: (200.0, 100.0),
        };
        let b = v.bounds();
        assert!(approx(b.x_min, 0.0) && approx(b.x_max, 2.0));
        assert!(approx(b.y_min, 1.5) && approx(b.y_max, 2.5));
    }

    #[test]
    fn pan_follows_pointer() {
        let mut v = View::default();
        let before = v.screen_to_world(100.0, 100.0);
        v.pan_by_pixels(30.0, -20.0);
        let after = v.screen_to_world(130.0, 80.0);
        assert!(approx(before.0, after.0) && approx(before.1, after.1));
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut v = View::default();
        let anchor = v.screen_to_world(123.0, 456.0);
        v.zoom_at(2.5, 123.0, 456.0);
        assert!(approx(v.scale, 150.0));
        let after = v.screen_to_world(123.0, 456.0);
        assert!(approx(anchor.0, after.0) && approx(anchor.1, after.1));
    }

    #[test]
    fn zoom_is_clamped_and_ignores_bad_factors() {
        let mut v = View::default();
        v.zoom_at(1e30, 0.0, 0.0);
        assert_eq!(v.scale, MAX_SCALE);
        v.zoom_at(1e-60, 0.0, 0.0);
        assert_eq!(v.scale, MIN_SCALE);
        let before = v;
        v.zoom_at(0.0, 0.0, 0.0);
        v.zoom_at(f64::NAN, 0.0, 0.0);
        assert_eq!(v, before);
    }

    #[test]
    fn nice_steps_are_1_2_5() {
        let cases = [
            (0.7, 1.0),
            (1.0, 1.0),
            (1.3, 2.0),
            (2.0, 2.0),
            (2.1, 5.0),
            (5.0, 5.0),
            (7.0, 10.0),
            (0.03, 0.05),
            (0.2, 0.2),
            (420.0, 500.0),
            (1.2e-7, 2e-7),
        ];
        for (raw, expected) in cases {
            assert!(approx(nice_step(raw), expected), "nice_step({raw})");
        }
        assert_eq!(nice_step(0.0), 1.0);
        assert_eq!(nice_step(-3.0), 1.0);
        assert_eq!(nice_step(f64::INFINITY), 1.0);
    }

    #[test]
    fn grid_steps_respect_min_spacing() {
        for scale in [0.001, 0.37, 1.0, 60.0, 333.0, 1e6] {
            let g = grid_steps(scale, 80.0);
            assert!(g.major * scale >= 80.0 - 1e-6, "scale {scale}");
            assert!(g.major * scale < 80.0 * 2.5 + 1e-6, "scale {scale}");
        }
    }

    #[test]
    fn minor_steps_subdivide_major() {
        assert!(approx(grid_steps(80.0, 80.0).minor, 0.2)); // major 1
        assert!(approx(grid_steps(40.0, 80.0).minor, 0.5)); // major 2
        assert!(approx(grid_steps(16.0, 80.0).minor, 1.0)); // major 5
    }

    #[test]
    fn ticks_cover_range() {
        let t: Vec<_> = ticks(-1.1, 2.5, 1.0).collect();
        assert_eq!(t, vec![(-1, -1.0), (0, 0.0), (1, 1.0), (2, 2.0)]);
        assert_eq!(ticks(0.1, 0.9, 1.0).count(), 0);
        assert_eq!(ticks(0.0, 1e9, 1e-3).count(), 0);
        assert_eq!(ticks(0.0, 1.0, 0.0).count(), 0);
        assert_eq!(ticks(1e17, 1e17 + 4.0, 1.0).count(), 0);
        assert_eq!(ticks(-1e300, -1e300, 1e-300).count(), 0);
    }

    #[test]
    fn center_is_clamped() {
        let mut v = View {
            scale: MIN_SCALE,
            ..View::default()
        };
        v.pan_by_pixels(-1e9, 1e9);
        assert_eq!(v.center, (MAX_CENTER, MAX_CENTER));
        v.pan_by_pixels(f64::INFINITY, 0.0);
        assert!(v.center.0.is_finite());
        v.zoom_at(1e-3, 1e9, -1e9);
        assert!(v.center.0.abs() <= MAX_CENTER && v.center.1.abs() <= MAX_CENTER);
    }

    #[test]
    fn label_digits_follow_zoom() {
        assert_eq!(label_digits(60.0), 2);
        assert_eq!(label_digits(1.0), 0);
        assert_eq!(label_digits(0.001), 0);
        assert_eq!(label_digits(1e6), 6);
        assert_eq!(label_digits(f64::NAN), 3);
    }

    #[test]
    fn tick_labels() {
        assert_eq!(format_tick(0.0, 1.0), "0");
        assert_eq!(format_tick(1e-17, 0.1), "0");
        assert_eq!(format_tick(3.0, 1.0), "3");
        assert_eq!(format_tick(-2.0, 1.0), "\u{2212}2");
        assert_eq!(format_tick(0.30000000000000004, 0.1), "0.3");
        assert_eq!(format_tick(0.25, 0.05), "0.25");
        assert_eq!(format_tick(1500.0, 500.0), "1500");
        assert_eq!(format_tick(2e7, 1e7), "2e7");
        assert_eq!(
            format_tick(-3.0000000000000004e-6, 1e-6),
            "\u{2212}3e\u{2212}6"
        );
    }

    #[test]
    fn box_zoom_fits_rect() {
        let mut v = View::default();
        // A 200x100 px box around the origin: 3.33 x 1.67 world units.
        assert!(v.fit_screen_rect((300.0, 250.0), (500.0, 350.0), true));
        assert!(approx(v.aspect, 1.0));
        assert!(approx(v.scale, 240.0), "{}", v.scale); // 800 / (200/60)
        assert!(approx(v.center.0, 0.0) && approx(v.center.1, 0.0));
        let mut u = View::default();
        assert!(u.fit_screen_rect((300.0, 250.0), (500.0, 350.0), false));
        assert!(approx(u.scale, 240.0));
        assert!(approx(u.scale_y(), 360.0)); // 600 / (100/60)
        let (x, y) = u.screen_to_world(800.0, 0.0);
        assert!(approx(x, 200.0 / 60.0 / 2.0) && approx(y, 100.0 / 60.0 / 2.0));
        assert!(!u.fit_screen_rect((1.0, 1.0), (3.0, 90.0), false));
    }

    #[test]
    fn plot_view_pads_and_covers() {
        let v = View::default();
        let p = v.plot_view(0.25).unwrap();
        assert!(approx(p.width_px(), 1200.0) && approx(p.height_px(), 900.0));
        assert!(v.covered_by(&p));
        let mut panned = v;
        panned.pan_by_pixels(150.0, -100.0);
        assert!(panned.covered_by(&p), "small pans reuse the padded layers");
        panned.pan_by_pixels(300.0, 0.0);
        assert!(!panned.covered_by(&p));
        let mut zoomed = v;
        zoomed.zoom_at(1.1, 400.0, 300.0);
        assert!(!zoomed.covered_by(&p), "zoom needs a resample");
        let tiny = View {
            size: (0.0, 10.0),
            ..View::default()
        };
        assert!(tiny.plot_view(0.25).is_none());
    }

    #[test]
    fn unlocked_aspect_scales_y() {
        let v = View {
            aspect: 2.0,
            ..View::default()
        };
        let (_, sy) = v.world_to_screen(0.0, 1.0);
        assert!(approx(sy, 300.0 - 120.0));
        let (_, wy) = v.screen_to_world(0.0, sy);
        assert!(approx(wy, 1.0));
    }
}
