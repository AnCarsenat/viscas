//! Geometric primitives in world coordinates and clipping helpers.

use std::ops::{Add, Mul, Sub};

/// A 2D point or vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    /// Linear interpolation `self + (other - self) * t`, computed from the
    /// nearer endpoint for accuracy when the endpoints differ hugely.
    pub fn lerp(self, other: Self, t: f64) -> Self {
        if t <= 0.5 {
            Self::new(
                self.x + (other.x - self.x) * t,
                self.y + (other.y - self.y) * t,
            )
        } else {
            let u = 1.0 - t;
            Self::new(
                other.x + (self.x - other.x) * u,
                other.y + (self.y - other.y) * u,
            )
        }
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;
    fn mul(self, k: f64) -> Self {
        Self::new(self.x * k, self.y * k)
    }
}

/// Axis-aligned rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x_min: f64,
    pub y_min: f64,
    pub x_max: f64,
    pub y_max: f64,
}

pub const OUT_LEFT: u8 = 1;
pub const OUT_RIGHT: u8 = 2;
pub const OUT_BOTTOM: u8 = 4;
pub const OUT_TOP: u8 = 8;

impl Rect {
    pub const fn new(x_min: f64, y_min: f64, x_max: f64, y_max: f64) -> Self {
        Self {
            x_min,
            y_min,
            x_max,
            y_max,
        }
    }

    pub fn width(&self) -> f64 {
        self.x_max - self.x_min
    }

    pub fn height(&self) -> f64 {
        self.y_max - self.y_min
    }

    pub fn center(&self) -> Vec2 {
        Vec2::new(
            0.5 * (self.x_min + self.x_max),
            0.5 * (self.y_min + self.y_max),
        )
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x_min && p.x <= self.x_max && p.y >= self.y_min && p.y <= self.y_max
    }

    /// Grow each side by `fraction` of the rectangle's size along that axis.
    pub fn expanded(&self, fraction: f64) -> Self {
        let dx = self.width() * fraction;
        let dy = self.height() * fraction;
        Self::new(
            self.x_min - dx,
            self.y_min - dy,
            self.x_max + dx,
            self.y_max + dy,
        )
    }

    /// Cohen–Sutherland outcode built from the `OUT_*` bits (0 means inside).
    pub fn outcode(&self, p: Vec2) -> u8 {
        let mut c = 0;
        if p.x < self.x_min {
            c |= OUT_LEFT;
        } else if p.x > self.x_max {
            c |= OUT_RIGHT;
        }
        if p.y < self.y_min {
            c |= OUT_BOTTOM;
        } else if p.y > self.y_max {
            c |= OUT_TOP;
        }
        c
    }
}

/// Stroke style hint for curves (strict inequalities are dashed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
}

/// Connected sequence of points. `closed` means the last point joins the first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polyline {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

impl Polyline {
    pub fn open(points: Vec<Vec2>) -> Self {
        Self {
            points,
            closed: false,
        }
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Clip against `rect`, splitting into several open polylines where the
    /// curve leaves the rectangle. A closed polyline fully inside stays closed.
    pub fn clipped(&self, rect: &Rect) -> Vec<Polyline> {
        if self.closed && self.points.iter().all(|&p| rect.contains(p)) {
            return vec![self.clone()];
        }
        let mut pts = self.points.clone();
        if self.closed
            && let Some(&first) = pts.first()
        {
            pts.push(first);
        }
        clip_polyline(&pts, rect)
            .into_iter()
            .map(Polyline::open)
            .collect()
    }
}

/// Simple polygon ring (implicitly closed), used for filled regions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polygon {
    pub points: Vec<Vec2>,
}

impl Polygon {
    pub fn new(points: Vec<Vec2>) -> Self {
        Self { points }
    }

    /// Signed area (positive for counter-clockwise rings in a y-up frame).
    pub fn signed_area(&self) -> f64 {
        let n = self.points.len();
        if n < 3 {
            return 0.0;
        }
        let mut s = 0.0;
        for i in 0..n {
            let a = self.points[i];
            let b = self.points[(i + 1) % n];
            s += a.x * b.y - b.x * a.y;
        }
        0.5 * s
    }

    pub fn area(&self) -> f64 {
        self.signed_area().abs()
    }
}

/// Draw primitive in world coordinates. The UI maps it through
/// [`crate::View::transform`].
#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Polyline { line: Polyline, style: LineStyle },
    Polygon(Polygon),
    Points(Vec<Vec2>),
}

/// Liang–Barsky: parametric range of segment `p0→p1` inside `r`.
fn clip_segment(p0: Vec2, p1: Vec2, r: &Rect) -> Option<(f64, f64)> {
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    let mut t0 = 0.0_f64;
    let mut t1 = 1.0_f64;
    for (p, q) in [
        (-dx, p0.x - r.x_min),
        (dx, r.x_max - p0.x),
        (-dy, p0.y - r.y_min),
        (dy, r.y_max - p0.y),
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
    (t0 < t1).then_some((t0, t1))
}

/// Clip an open polyline to `rect`, returning the visible pieces (each with
/// at least two points). Crossing points are inserted on the boundary.
pub fn clip_polyline(points: &[Vec2], rect: &Rect) -> Vec<Vec<Vec2>> {
    let mut out = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    for w in points.windows(2) {
        let (p0, p1) = (w[0], w[1]);
        match clip_segment(p0, p1, rect) {
            Some((t0, t1)) => {
                let a = if t0 > 0.0 { p0.lerp(p1, t0) } else { p0 };
                let b = if t1 < 1.0 { p0.lerp(p1, t1) } else { p1 };
                if cur.is_empty() || t0 > 0.0 {
                    flush(&mut out, &mut cur);
                    cur.push(a);
                }
                cur.push(b);
                if t1 < 1.0 {
                    flush(&mut out, &mut cur);
                }
            }
            None => flush(&mut out, &mut cur),
        }
    }
    flush(&mut out, &mut cur);
    out
}

fn flush(out: &mut Vec<Vec<Vec2>>, cur: &mut Vec<Vec2>) {
    if cur.len() >= 2 {
        out.push(std::mem::take(cur));
    } else {
        cur.clear();
    }
}

/// Clamp the y coordinates of a polyline into `[lo, hi]`, inserting the exact
/// crossing points first so the clamped shape matches the original inside the
/// band. Used to build fill polygons for inequalities.
pub fn clamp_polyline_y(points: &[Vec2], lo: f64, hi: f64) -> Vec<Vec2> {
    let clamp = |p: Vec2| Vec2::new(p.x, p.y.clamp(lo, hi));
    let mut out = Vec::with_capacity(points.len() + 4);
    let Some(&first) = points.first() else {
        return out;
    };
    out.push(clamp(first));
    for w in points.windows(2) {
        let (p, q) = (w[0], w[1]);
        let dy = q.y - p.y;
        if dy != 0.0 {
            let mut ts = [f64::NAN; 2];
            for (k, level) in [lo, hi].into_iter().enumerate() {
                let t = (level - p.y) / dy;
                if t > 0.0 && t < 1.0 {
                    ts[k] = t;
                }
            }
            if ts[0] > ts[1] {
                ts.swap(0, 1);
            }
            for t in ts {
                if t.is_finite() {
                    out.push(clamp(p.lerp(q, t)));
                }
            }
        }
        out.push(clamp(q));
    }
    out
}

/// Same as [`clamp_polyline_y`] but for the x coordinate.
pub fn clamp_polyline_x(points: &[Vec2], lo: f64, hi: f64) -> Vec<Vec2> {
    let swapped: Vec<Vec2> = points.iter().map(|p| Vec2::new(p.y, p.x)).collect();
    clamp_polyline_y(&swapped, lo, hi)
        .into_iter()
        .map(|p| Vec2::new(p.y, p.x))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> Rect {
        Rect::new(0.0, 0.0, 1.0, 1.0)
    }

    #[test]
    fn vec_ops() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(3.0, -1.0);
        assert_eq!(a + b, Vec2::new(4.0, 1.0));
        assert_eq!(b - a, Vec2::new(2.0, -3.0));
        assert_eq!(a * 2.0, Vec2::new(2.0, 4.0));
        assert_eq!(a.lerp(b, 0.5), Vec2::new(2.0, 0.5));
        assert!(!Vec2::new(f64::NAN, 0.0).is_finite());
    }

    #[test]
    fn outcodes() {
        let r = unit();
        assert_eq!(r.outcode(Vec2::new(0.5, 0.5)), 0);
        assert_eq!(r.outcode(Vec2::new(-1.0, 2.0)), OUT_LEFT | OUT_TOP);
        assert_eq!(r.outcode(Vec2::new(2.0, -1.0)), OUT_RIGHT | OUT_BOTTOM);
    }

    #[test]
    fn clip_inside_untouched() {
        let pts = vec![
            Vec2::new(0.1, 0.1),
            Vec2::new(0.5, 0.9),
            Vec2::new(0.9, 0.2),
        ];
        assert_eq!(clip_polyline(&pts, &unit()), vec![pts]);
    }

    #[test]
    fn clip_splits_on_exit_and_reentry() {
        let pts = vec![
            Vec2::new(0.1, 0.5),
            Vec2::new(0.5, 2.0),
            Vec2::new(0.9, 0.5),
        ];
        let pieces = clip_polyline(&pts, &unit());
        assert_eq!(pieces.len(), 2);
        assert!((pieces[0].last().unwrap().y - 1.0).abs() < 1e-12);
        assert!((pieces[1][0].y - 1.0).abs() < 1e-12);
    }

    #[test]
    fn clip_segment_crossing_whole_rect() {
        let pts = vec![Vec2::new(-1.0, 0.5), Vec2::new(2.0, 0.5)];
        let pieces = clip_polyline(&pts, &unit());
        assert_eq!(pieces, vec![vec![Vec2::new(0.0, 0.5), Vec2::new(1.0, 0.5)]]);
    }

    #[test]
    fn clip_fully_outside() {
        let pts = vec![Vec2::new(-1.0, 2.0), Vec2::new(2.0, 2.0)];
        assert!(clip_polyline(&pts, &unit()).is_empty());
    }

    #[test]
    fn closed_polyline_inside_stays_closed() {
        let line = Polyline {
            points: vec![
                Vec2::new(0.2, 0.2),
                Vec2::new(0.8, 0.2),
                Vec2::new(0.5, 0.8),
            ],
            closed: true,
        };
        let c = line.clipped(&unit());
        assert_eq!(c.len(), 1);
        assert!(c[0].closed);
    }

    #[test]
    fn clamp_inserts_crossings() {
        let pts = vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 10.0)];
        let c = clamp_polyline_y(&pts, -1.0, 5.0);
        assert_eq!(c.len(), 3);
        assert!((c[1].x - 0.5).abs() < 1e-12 && c[1].y == 5.0);
        assert_eq!(c[2], Vec2::new(1.0, 5.0));
        let cx = clamp_polyline_x(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 1.0)], -1.0, 5.0);
        assert!((cx[1].y - 0.5).abs() < 1e-12 && cx[1].x == 5.0);
    }

    #[test]
    fn polygon_area() {
        let sq = Polygon::new(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(0.0, 2.0),
        ]);
        assert_eq!(sq.signed_area(), 4.0);
        assert_eq!(sq.area(), 4.0);
    }
}
