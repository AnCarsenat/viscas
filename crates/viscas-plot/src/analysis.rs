//! Numeric feature detection for explicit functions (click-to-show-point):
//! roots, local extrema and intersections inside a range.
//!
//! Each search samples the range uniformly, brackets candidates and refines
//! them (Brent for sign changes, golden-section for extrema and touching
//! roots). Candidates produced by poles or jumps (`1/x`, `tan`, `floor`) are
//! rejected by checking that the refined value really behaves like a root or
//! a smooth extremum.

use crate::cancel::{Budget, CancelToken, Cancelled};
use crate::geom::Vec2;
use crate::view::View;

/// What a [`Feature`] point is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Root,
    Minimum,
    Maximum,
    Intersection,
    YIntercept,
}

/// A notable point on a curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feature {
    pub pos: Vec2,
    pub kind: FeatureKind,
}

/// Brent's method on a bracket `[a, b]` with a sign change. Returns `None`
/// without a sign change or if `f` is not finite at an iterate.
pub fn brent_root<F>(f: &F, a: f64, b: f64, xtol: f64, max_iter: usize) -> Option<f64>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let (mut a, mut b) = (a, b);
    let (mut fa, mut fb) = (f(a), f(b));
    if !(fa.is_finite() && fb.is_finite()) {
        return None;
    }
    if fa == 0.0 {
        return Some(a);
    }
    if fb == 0.0 {
        return Some(b);
    }
    if (fa > 0.0) == (fb > 0.0) {
        return None;
    }
    let (mut c, mut fc) = (b, fb);
    let mut d = b - a;
    let mut e = d;
    for _ in 0..max_iter {
        if (fb > 0.0) == (fc > 0.0) {
            c = a;
            fc = fa;
            d = b - a;
            e = d;
        }
        if fc.abs() < fb.abs() {
            a = b;
            b = c;
            c = a;
            fa = fb;
            fb = fc;
            fc = fa;
        }
        let tol1 = 2.0 * f64::EPSILON * b.abs() + 0.5 * xtol;
        let xm = 0.5 * (c - b);
        if xm.abs() <= tol1 || fb == 0.0 {
            return Some(b);
        }
        if e.abs() >= tol1 && fa.abs() > fb.abs() {
            // Inverse quadratic interpolation / secant.
            let s = fb / fa;
            let (mut p, mut q);
            if a == c {
                p = 2.0 * xm * s;
                q = 1.0 - s;
            } else {
                let qq = fa / fc;
                let r = fb / fc;
                p = s * (2.0 * xm * qq * (qq - r) - (b - a) * (r - 1.0));
                q = (qq - 1.0) * (r - 1.0) * (s - 1.0);
            }
            if p > 0.0 {
                q = -q;
            } else {
                p = -p;
            }
            if 2.0 * p < (3.0 * xm * q - (tol1 * q).abs()).min((e * q).abs()) {
                e = d;
                d = p / q;
            } else {
                d = xm;
                e = d;
            }
        } else {
            d = xm;
            e = d;
        }
        a = b;
        fa = fb;
        b += if d.abs() > tol1 { d } else { tol1.copysign(xm) };
        fb = f(b);
        if !fb.is_finite() {
            return None;
        }
    }
    Some(b)
}

/// Golden-section search for a minimum of `f` on `[a, b]` (NaN counts as +∞).
pub fn golden_min<F>(f: &F, a: f64, b: f64, xtol: f64) -> f64
where
    F: Fn(f64) -> f64 + ?Sized,
{
    const R: f64 = 0.618_033_988_749_894_9;
    let g = |x: f64| {
        let v = f(x);
        if v.is_nan() { f64::INFINITY } else { v }
    };
    let (mut a, mut b) = if a <= b { (a, b) } else { (b, a) };
    let mut c = b - R * (b - a);
    let mut d = a + R * (b - a);
    let (mut fc, mut fd) = (g(c), g(d));
    for _ in 0..200 {
        if b - a <= xtol.max(f64::EPSILON * (a.abs() + b.abs())) {
            break;
        }
        if fc < fd {
            b = d;
            d = c;
            fd = fc;
            c = b - R * (b - a);
            fc = g(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + R * (b - a);
            fd = g(d);
        }
    }
    0.5 * (a + b)
}

struct Samples {
    xs: Vec<f64>,
    ys: Vec<f64>,
}

fn sample<F>(f: &F, x0: f64, x1: f64, n: usize, cancel: &CancelToken) -> Result<Samples, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let n = n.max(2);
    let mut budget = Budget::new(usize::MAX, cancel);
    let mut xs = Vec::with_capacity(n + 1);
    let mut ys = Vec::with_capacity(n + 1);
    for i in 0..=n {
        budget.tick()?;
        let x = if i == n {
            x1
        } else {
            x0 + (x1 - x0) * (i as f64 / n as f64)
        };
        xs.push(x);
        ys.push(f(x));
    }
    cancel.check()?;
    Ok(Samples { xs, ys })
}

fn dedup_sorted(v: &mut Vec<f64>, eps: f64) {
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() <= eps);
}

/// Roots of `f` in `[x0, x1]` from `samples` uniform intervals: sign changes
/// (refined with Brent, poles/jumps rejected) and touching roots such as
/// `x²` at 0 (local minima of `|f|` that reach zero).
pub fn find_roots<F>(
    f: &F,
    x0: f64,
    x1: f64,
    samples: usize,
    cancel: &CancelToken,
) -> Result<Vec<f64>, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    if !(x0.is_finite() && x1.is_finite() && x0 < x1) {
        return Ok(Vec::new());
    }
    let Samples { xs, ys } = sample(f, x0, x1, samples, cancel)?;
    let span = x1 - x0;
    let xtol = span * 1e-15;
    let max_abs = ys
        .iter()
        .filter(|y| y.is_finite())
        .fold(0.0_f64, |m, y| m.max(y.abs()));
    let mut roots = Vec::new();
    for i in 0..xs.len() {
        let y = ys[i];
        if !y.is_finite() {
            continue;
        }
        if y == 0.0 {
            roots.push(xs[i]);
            continue;
        }
        if let Some(&yn) = ys.get(i + 1)
            && yn.is_finite()
            && yn != 0.0
            && (y > 0.0) != (yn > 0.0)
            && let Some(r) = brent_root(f, xs[i], xs[i + 1], xtol, 200)
        {
            let fr = f(r);
            // A real root drives |f| to ~0; poles blow up and jumps stay put.
            if fr.is_finite() && fr.abs() <= 1e-6 * y.abs().max(yn.abs()) {
                roots.push(r);
            }
        }
        if i > 0
            && let Some(&yn) = ys.get(i + 1)
        {
            let yp = ys[i - 1];
            let same_sign = yp.is_finite() && yn.is_finite() && (yp > 0.0) == (y > 0.0);
            let same_sign = same_sign && (yn > 0.0) == (y > 0.0) && yp != 0.0 && yn != 0.0;
            if same_sign && y.abs() <= yp.abs() && y.abs() < yn.abs() {
                let abs_f = |x: f64| f(x).abs();
                let xm = golden_min(&abs_f, xs[i - 1], xs[i + 1], xtol);
                if abs_f(xm) <= 1e-10 * max_abs.max(1.0) {
                    roots.push(xm);
                }
            }
        }
    }
    dedup_sorted(&mut roots, span * 1e-9);
    Ok(roots)
}

/// Local minima and maxima of `f` strictly inside `[x0, x1]`.
pub fn find_extrema<F>(
    f: &F,
    x0: f64,
    x1: f64,
    samples: usize,
    cancel: &CancelToken,
) -> Result<Vec<Feature>, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    if !(x0.is_finite() && x1.is_finite() && x0 < x1) {
        return Ok(Vec::new());
    }
    let Samples { xs, ys } = sample(f, x0, x1, samples, cancel)?;
    let xtol = (x1 - x0) * 1e-13;
    let mut out: Vec<Feature> = Vec::new();
    for i in 1..xs.len().saturating_sub(1) {
        let (ya, yb, yc) = (ys[i - 1], ys[i], ys[i + 1]);
        if !(ya.is_finite() && yb.is_finite() && yc.is_finite()) {
            continue;
        }
        let kind = if yb > ya && yb > yc {
            FeatureKind::Maximum
        } else if yb < ya && yb < yc {
            FeatureKind::Minimum
        } else {
            continue;
        };
        let sign = if kind == FeatureKind::Maximum {
            -1.0
        } else {
            1.0
        };
        let g = |x: f64| sign * f(x);
        let xm = golden_min(&g, xs[i - 1], xs[i + 1], xtol);
        let ym = f(xm);
        // Reject poles: near a smooth (or cusp) extremum f barely moves.
        let h = (xs[i + 1] - xs[i - 1]) * 1e-4;
        let scale = (ya - yb).abs() + (yc - yb).abs();
        let smooth = [xm - h, xm + h].iter().all(|&x| {
            let v = f(x);
            v.is_finite() && (v - ym).abs() <= 1e-3 * scale
        });
        if ym.is_finite() && smooth {
            out.push(Feature {
                pos: Vec2::new(xm, ym),
                kind,
            });
        }
    }
    Ok(out)
}

/// Intersections of `y = f(x)` and `y = g(x)` in `[x0, x1]`.
pub fn find_intersections<F, G>(
    f: &F,
    g: &G,
    x0: f64,
    x1: f64,
    samples: usize,
    cancel: &CancelToken,
) -> Result<Vec<Vec2>, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
    G: Fn(f64) -> f64 + ?Sized,
{
    let h = |x: f64| f(x) - g(x);
    let xs = find_roots(&h, x0, x1, samples, cancel)?;
    Ok(xs
        .into_iter()
        .map(|x| Vec2::new(x, f(x)))
        .filter(|p| p.is_finite())
        .collect())
}

/// Roots, extrema and the y-intercept of `y = f(x)` visible in the view,
/// sampled at pixel resolution.
pub fn explicit_features<F>(
    f: &F,
    view: &View,
    cancel: &CancelToken,
) -> Result<Vec<Feature>, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let b = view.bounds();
    let n = view.width_px().ceil() as usize;
    let mut out: Vec<Feature> = find_roots(f, b.x_min, b.x_max, n, cancel)?
        .into_iter()
        .map(|x| Feature {
            pos: Vec2::new(x, 0.0),
            kind: FeatureKind::Root,
        })
        .collect();
    out.extend(find_extrema(f, b.x_min, b.x_max, n, cancel)?);
    if b.x_min <= 0.0 && 0.0 <= b.x_max {
        let y = f(0.0);
        if y.is_finite() {
            out.push(Feature {
                pos: Vec2::new(0.0, y),
                kind: FeatureKind::YIntercept,
            });
        }
    }
    out.retain(|ft| b.contains(ft.pos));
    Ok(out)
}

/// The feature closest to a screen position, within `radius_px` pixels.
pub fn nearest_feature(
    features: &[Feature],
    view: &View,
    screen: Vec2,
    radius_px: f64,
) -> Option<Feature> {
    features
        .iter()
        .map(|ft| ((view.world_to_screen(ft.pos) - screen).length(), ft))
        .filter(|(d, _)| *d <= radius_px)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, ft)| *ft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    fn none() -> CancelToken {
        CancelToken::none()
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn brent_finds_sqrt2_and_dottie() {
        let r = brent_root(&|x: f64| x * x - 2.0, 0.0, 2.0, 1e-15, 100).unwrap();
        assert!(close(r, 2f64.sqrt(), 1e-14));
        let r = brent_root(&|x: f64| x.cos() - x, 0.0, 1.0, 1e-15, 100).unwrap();
        assert!(close(r, 0.739_085_133_215_160_6, 1e-14));
        assert!(brent_root(&|x: f64| x * x + 1.0, -1.0, 1.0, 1e-12, 100).is_none());
        assert_eq!(brent_root(&|x: f64| x, 0.0, 1.0, 1e-12, 100), Some(0.0));
    }

    #[test]
    fn golden_finds_minimum() {
        let x = golden_min(&|x: f64| (x - 1.25).powi(2) + 3.0, -4.0, 4.0, 1e-12);
        assert!(close(x, 1.25, 1e-7));
    }

    #[test]
    fn sine_roots() {
        let r = find_roots(&f64::sin, -10.0, 10.0, 1000, &none()).unwrap();
        assert_eq!(r.len(), 7);
        for (k, x) in (-3..=3).zip(&r) {
            assert!(close(*x, k as f64 * PI, 1e-12), "{x}");
        }
    }

    #[test]
    fn poles_and_jumps_are_not_roots() {
        assert!(
            find_roots(&|x: f64| 1.0 / x, -5.0, 5.0, 999, &none())
                .unwrap()
                .is_empty()
        );
        assert!(
            find_roots(&|x: f64| x.floor() + 0.5, -3.3, 3.3, 1000, &none())
                .unwrap()
                .is_empty()
        );
        let r = find_roots(&f64::tan, -5.0, 5.0, 1000, &none()).unwrap();
        assert_eq!(r.len(), 3);
        assert!(close(r[0], -PI, 1e-12) && close(r[1], 0.0, 1e-12) && close(r[2], PI, 1e-12));
    }

    #[test]
    fn touching_roots() {
        let r = find_roots(&|x: f64| x * x, -3.0, 3.1, 1000, &none()).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].abs() < 1e-6);
        let f = |x: f64| (x - 1.0).powi(2) * (x + 2.0);
        let r = find_roots(&f, -5.0, 5.0, 777, &none()).unwrap();
        assert_eq!(r.len(), 2);
        assert!(close(r[0], -2.0, 1e-12) && close(r[1], 1.0, 1e-6));
        assert!(
            find_roots(&|x: f64| x * x + 1e-3, -3.0, 3.1, 1000, &none())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn sine_extrema() {
        let e = find_extrema(&f64::sin, -10.0, 10.0, 1000, &none()).unwrap();
        let maxima: Vec<_> = e
            .iter()
            .filter(|f| f.kind == FeatureKind::Maximum)
            .collect();
        let minima: Vec<_> = e
            .iter()
            .filter(|f| f.kind == FeatureKind::Minimum)
            .collect();
        assert_eq!(maxima.len(), 3);
        assert_eq!(minima.len(), 3);
        assert!(maxima.iter().any(|f| close(f.pos.x, FRAC_PI_2, 1e-6)));
        assert!(maxima.iter().all(|f| close(f.pos.y, 1.0, 1e-12)));
        assert!(minima.iter().all(|f| close(f.pos.y, -1.0, 1e-12)));
    }

    #[test]
    fn cubic_and_cusp_extrema() {
        let e = find_extrema(&|x: f64| x * x * x - 3.0 * x, -3.0, 3.0, 600, &none()).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].kind, FeatureKind::Maximum);
        assert!(close(e[0].pos.x, -1.0, 1e-6) && close(e[0].pos.y, 2.0, 1e-10));
        assert_eq!(e[1].kind, FeatureKind::Minimum);
        let e = find_extrema(&f64::abs, -1.0, 1.3, 100, &none()).unwrap();
        assert_eq!(e.len(), 1);
        assert!(e[0].pos.x.abs() < 1e-9);
    }

    #[test]
    fn tan_has_no_extrema() {
        let e = find_extrema(&f64::tan, -5.0, 5.0, 1000, &none()).unwrap();
        assert!(e.is_empty(), "{e:?}");
        let e = find_extrema(&|x: f64| 1.0 / (x * x), -5.0, 5.0, 1000, &none()).unwrap();
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn intersections() {
        let p = find_intersections(&|x: f64| x * x, &|x: f64| x + 2.0, -5.0, 5.0, 1000, &none())
            .unwrap();
        assert_eq!(p.len(), 2);
        assert!(close(p[0].x, -1.0, 1e-12) && close(p[0].y, 1.0, 1e-12));
        assert!(close(p[1].x, 2.0, 1e-12) && close(p[1].y, 4.0, 1e-12));
        let p = find_intersections(&f64::sin, &f64::cos, 0.0, 2.0 * PI, 500, &none()).unwrap();
        assert_eq!(p.len(), 2);
        assert!(close(p[0].x, FRAC_PI_4, 1e-12) && close(p[1].x, 5.0 * FRAC_PI_4, 1e-12));
        let p = find_intersections(
            &|x: f64| 1.0 / x,
            &|x: f64| -1.0 / x,
            -3.0,
            3.0,
            601,
            &none(),
        )
        .unwrap();
        assert!(p.is_empty());
    }

    #[test]
    fn features_in_view_and_nearest() {
        let v = View::new(Rect::new(-3.0, -2.0, 3.0, 2.0), 600.0, 400.0);
        let fs = explicit_features(&|x: f64| x * x - 1.0, &v, &none()).unwrap();
        let count = |k| fs.iter().filter(|f| f.kind == k).count();
        assert_eq!(count(FeatureKind::Root), 2);
        assert_eq!(count(FeatureKind::Minimum), 1);
        assert_eq!(count(FeatureKind::YIntercept), 1);
        let near_root = v.world_to_screen(Vec2::new(1.0, 0.0)) + Vec2::new(3.0, -2.0);
        let hit = nearest_feature(&fs, &v, near_root, 8.0).unwrap();
        assert_eq!(hit.kind, FeatureKind::Root);
        assert!(close(hit.pos.x, 1.0, 1e-12));
        assert!(nearest_feature(&fs, &v, Vec2::new(5.0, 5.0), 8.0).is_none());
    }

    #[test]
    fn analysis_cancel_and_bad_range() {
        let t = CancelToken::new();
        t.cancel();
        assert_eq!(find_roots(&f64::sin, 0.0, 1.0, 100, &t), Err(Cancelled));
        assert!(
            find_roots(&f64::sin, 1.0, 0.0, 100, &none())
                .unwrap()
                .is_empty()
        );
        assert!(
            find_extrema(&f64::sin, f64::NAN, 1.0, 100, &none())
                .unwrap()
                .is_empty()
        );
    }
}
