//! Adaptive sampling of explicit, parametric and polar curves.
//!
//! Everything is reduced to a parametric curve `t ↦ (x(t), y(t))`:
//! `y = f(x)` is `t ↦ (t, f(t))`, `x = f(y)` is `t ↦ (f(t), t)` and polar
//! `r = f(θ)` is `θ ↦ (r cos θ, r sin θ)`.
//!
//! Algorithm:
//! 1. Uniform initial samples over the parameter range.
//! 2. Each interval is recursively bisected while the midpoint deviates from
//!    the chord by more than `tolerance_px` or the chord is longer than
//!    `max_chord_px` (both measured in screen pixels). Once an interval has
//!    been halved at least twice, it is no longer refined if its three
//!    samples all lie outside the view on the same side (the two forced
//!    levels catch narrow excursions into the view between samples).
//! 3. Non-finite samples mark domain gaps; the domain boundary is located by
//!    bisection to full `f64` precision so curves like `sqrt(x)` reach their
//!    endpoint. Tiny gaps whose sides meet on screen (removable
//!    singularities such as `sin(x)/x` at 0) are bridged.
//! 4. A chord still too long at `max_depth` is classified by bisecting
//!    towards the larger jump: if the jump shrinks below the tolerance the
//!    curve is continuous (steep), otherwise it is a discontinuity
//!    (asymptote or jump) and the polyline is split there.
//!
//! `max_evals` is a hard cap that includes the initial pass; when it is hit
//! the result is returned with `truncated = true`.

use crate::cancel::{Budget, Cancelled, Sampled};
use crate::geom::{Polyline, Rect, Vec2};
use crate::options::PlotOptions;
use crate::view::View;

/// Polylines plus the truncation flag.
pub type Curves = Sampled<Vec<Polyline>>;

/// Number of levels an interval is always refined before off-view culling.
const MIN_CULL_DEPTH: u32 = 2;

#[derive(Clone, Copy, Debug)]
struct Sample {
    t: f64,
    p: Vec2,
    ok: bool,
}

struct Sampler<'s, 'c, F: ?Sized> {
    f: &'s F,
    sx: f64,
    sy: f64,
    /// View plus a 2 px border; refinement stops outside it.
    refine_rect: Rect,
    tol: f64,
    max_chord: f64,
    max_depth: u32,
    budget: &'s mut Budget<'c>,
    lines: Vec<Vec<Sample>>,
    cur: Vec<Sample>,
}

impl<'s, 'c, F> Sampler<'s, 'c, F>
where
    F: Fn(f64) -> (f64, f64) + ?Sized,
{
    fn new(f: &'s F, view: &View, opts: &PlotOptions, budget: &'s mut Budget<'c>) -> Self {
        Self {
            f,
            sx: 1.0 / view.units_per_px_x(),
            sy: 1.0 / view.units_per_px_y(),
            refine_rect: {
                let b = view.bounds();
                let (mx, my) = (2.0 * view.units_per_px_x(), 2.0 * view.units_per_px_y());
                Rect::new(b.x_min - mx, b.y_min - my, b.x_max + mx, b.y_max + my)
            },
            tol: opts.tolerance_px.max(1e-3),
            max_chord: opts.max_chord_px.max(opts.tolerance_px).max(1e-3),
            max_depth: opts.max_depth,
            budget,
            lines: Vec::new(),
            cur: Vec::new(),
        }
    }

    /// Evaluate at `t`, or `None` when the budget is exhausted.
    #[inline]
    fn eval(&mut self, t: f64) -> Result<Option<Sample>, Cancelled> {
        if !self.budget.take()? {
            return Ok(None);
        }
        let (x, y) = (self.f)(t);
        let p = Vec2::new(x, y);
        Ok(Some(Sample {
            t,
            p,
            ok: p.is_finite(),
        }))
    }

    fn dist_px(&self, a: Vec2, b: Vec2) -> f64 {
        ((a.x - b.x) * self.sx).hypot((a.y - b.y) * self.sy)
    }

    /// Screen distance from `p` to the segment `a b`.
    fn seg_dist_px(&self, p: Vec2, a: Vec2, b: Vec2) -> f64 {
        let (px, py) = ((p.x - a.x) * self.sx, (p.y - a.y) * self.sy);
        let (bx, by) = ((b.x - a.x) * self.sx, (b.y - a.y) * self.sy);
        let l2 = bx * bx + by * by;
        let t = if l2 > 0.0 {
            ((px * bx + py * by) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (px - t * bx).hypot(py - t * by)
    }

    fn emit(&mut self, s: Sample) {
        self.cur.push(s);
    }

    fn brk(&mut self) {
        if !self.cur.is_empty() {
            self.lines.push(std::mem::take(&mut self.cur));
        }
    }

    fn run(&mut self, t0: f64, t1: f64, n: usize) -> Result<(), Cancelled> {
        // Keep at least half of the budget for refinement.
        let n = n.min(self.budget.remaining() / 2).max(1);
        let Some(mut prev) = self.eval(t0)? else {
            return Ok(());
        };
        if prev.ok {
            self.emit(prev);
        }
        for i in 1..=n {
            let t = if i == n {
                t1
            } else {
                t0 + (t1 - t0) * (i as f64 / n as f64)
            };
            let Some(s) = self.eval(t)? else {
                break;
            };
            self.segment(prev, s, 0)?;
            if s.ok {
                self.emit(s);
            }
            prev = s;
        }
        self.brk();
        Ok(())
    }

    /// Refine the open interval between `a` (already emitted if finite) and
    /// `b` (emitted by the caller if finite).
    fn segment(&mut self, a: Sample, b: Sample, depth: u32) -> Result<(), Cancelled> {
        match (a.ok, b.ok) {
            (false, false) => {
                // Probe once so narrow islands of definition are not lost.
                if depth == 0
                    && let Some(m) = self.eval(0.5 * (a.t + b.t))?
                    && m.ok
                {
                    self.segment(a, m, depth + 1)?;
                    self.emit(m);
                    self.segment(m, b, depth + 1)?;
                }
                Ok(())
            }
            (true, false) => {
                let e = self.boundary(a, b)?;
                if e.t != a.t {
                    self.segment(a, e, depth)?;
                    self.emit(e);
                }
                self.brk();
                Ok(())
            }
            (false, true) => {
                let s = self.boundary(b, a)?;
                self.brk();
                if s.t != b.t {
                    self.emit(s);
                    self.segment(s, b, depth)?;
                }
                Ok(())
            }
            (true, true) => self.refine(a, b, depth),
        }
    }

    fn refine(&mut self, a: Sample, b: Sample, depth: u32) -> Result<(), Cancelled> {
        let tm = 0.5 * (a.t + b.t);
        if tm <= a.t || tm >= b.t {
            return Ok(());
        }
        let Some(m) = self.eval(tm)? else {
            return Ok(());
        };
        if !m.ok {
            self.segment(a, m, depth + 1)?;
            return self.segment(m, b, depth + 1);
        }
        let clip = self.refine_rect;
        if depth >= MIN_CULL_DEPTH && clip.outcode(a.p) & clip.outcode(m.p) & clip.outcode(b.p) != 0
        {
            return Ok(());
        }
        let dev = self.seg_dist_px(m.p, a.p, b.p);
        let chord = self.dist_px(a.p, b.p);
        if dev <= self.tol && chord <= self.max_chord {
            self.emit(m);
            return Ok(());
        }
        if depth >= self.max_depth || self.budget.exhausted() {
            if chord > self.max_chord
                && !self.budget.exhausted()
                && let Some((l, r)) = self.discontinuity(a, b)?
            {
                if l.t != a.t {
                    self.emit(l);
                }
                self.brk();
                if r.t != b.t {
                    self.emit(r);
                }
                return Ok(());
            }
            self.emit(m);
            return Ok(());
        }
        self.refine(a, m, depth + 1)?;
        self.emit(m);
        self.refine(m, b, depth + 1)
    }

    /// Bisect between a finite and a non-finite sample; returns the finite
    /// sample closest to the domain boundary (as close as the budget allows).
    fn boundary(&mut self, good: Sample, bad: Sample) -> Result<Sample, Cancelled> {
        let (mut g, mut b) = (good, bad);
        for _ in 0..64 {
            let tm = 0.5 * (g.t + b.t);
            if tm == g.t || tm == b.t {
                break;
            }
            let Some(m) = self.eval(tm)? else {
                break;
            };
            if m.ok {
                g = m;
            } else {
                b = m;
            }
        }
        Ok(g)
    }

    /// Decide whether a long chord hides a discontinuity. Returns the left and
    /// right limit samples if it does. If the budget runs out the chord is
    /// conservatively treated as a discontinuity (no vertical connector).
    fn discontinuity(
        &mut self,
        a: Sample,
        b: Sample,
    ) -> Result<Option<(Sample, Sample)>, Cancelled> {
        let (mut a, mut b) = (a, b);
        for _ in 0..80 {
            if self.dist_px(a.p, b.p) <= self.tol {
                return Ok(None);
            }
            let tm = 0.5 * (a.t + b.t);
            if tm <= a.t || tm >= b.t {
                break;
            }
            let Some(m) = self.eval(tm)? else {
                break;
            };
            if !m.ok {
                let l = self.boundary(a, m)?;
                let r = self.boundary(b, m)?;
                return Ok(Some((l, r)));
            }
            if self.dist_px(a.p, m.p) >= self.dist_px(m.p, b.p) {
                b = m;
            } else {
                a = m;
            }
        }
        Ok(Some((a, b)))
    }

    /// Bridge removable gaps and convert to polylines.
    fn finish(mut self, t_span: f64) -> Vec<Polyline> {
        self.brk();
        let eps = t_span.abs() * 1e-9;
        let mut merged: Vec<Vec<Sample>> = Vec::new();
        for line in std::mem::take(&mut self.lines) {
            if let Some(last) = merged.last_mut() {
                let e = last[last.len() - 1];
                let s = line[0];
                if s.t - e.t <= eps && self.dist_px(e.p, s.p) <= self.tol {
                    last.extend(line);
                    continue;
                }
            }
            merged.push(line);
        }
        merged
            .into_iter()
            .filter(|l| l.len() >= 2)
            .map(|l| Polyline::open(l.into_iter().map(|s| s.p).collect()))
            .collect()
    }
}

fn normalize_range(t0: f64, t1: f64) -> Option<(f64, f64)> {
    if !(t0.is_finite() && t1.is_finite()) || t0 == t1 {
        return None;
    }
    Some(if t0 < t1 { (t0, t1) } else { (t1, t0) })
}

#[allow(clippy::too_many_arguments)]
fn sample_with<F>(
    f: &F,
    t0: f64,
    t1: f64,
    n: usize,
    view: &View,
    opts: &PlotOptions,
    budget: &mut Budget<'_>,
) -> Result<Vec<Polyline>, Cancelled>
where
    F: Fn(f64) -> (f64, f64) + ?Sized,
{
    let mut s = Sampler::new(f, view, opts, budget);
    s.run(t0, t1, n)?;
    Ok(s.finish(t1 - t0))
}

fn clip_all(c: Curves, view: &View, opts: &PlotOptions) -> Curves {
    let rect = view.bounds().expanded(opts.clip_margin.max(0.0));
    Sampled {
        data: c.data.iter().flat_map(|l| l.clipped(&rect)).collect(),
        truncated: c.truncated,
    }
}

fn parametric_samples(opts: &PlotOptions) -> usize {
    opts.parametric_samples.clamp(2, 100_000)
}

/// Adaptive samples of a parametric curve over `t_range`, unclipped (points
/// may lie far outside the view). A single polyline whose ends meet on screen
/// is marked `closed`.
pub fn sample_parametric<F>(
    f: &F,
    t_range: (f64, f64),
    view: &View,
    opts: &PlotOptions,
) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> (f64, f64) + ?Sized,
{
    let mut budget = Budget::new(opts.max_evals, &opts.cancel);
    let Some((t0, t1)) = normalize_range(t_range.0, t_range.1) else {
        return Ok(Curves::default());
    };
    let n = parametric_samples(opts);
    let mut lines = sample_with(f, t0, t1, n, view, opts, &mut budget)?;
    if let [line] = lines.as_mut_slice()
        && line.points.len() > 3
    {
        let (a, b) = (line.points[0], line.points[line.points.len() - 1]);
        let d = ((a.x - b.x) / view.units_per_px_x()).hypot((a.y - b.y) / view.units_per_px_y());
        if d <= opts.tolerance_px {
            line.points.pop();
            line.closed = true;
        }
    }
    Ok(Sampled {
        data: lines,
        truncated: budget.truncated(),
    })
}

/// Parametric curve `(x(t), y(t))`, clipped to the view (plus margin).
pub fn plot_parametric<F>(
    f: &F,
    t_range: (f64, f64),
    view: &View,
    opts: &PlotOptions,
) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> (f64, f64) + ?Sized,
{
    Ok(clip_all(
        sample_parametric(f, t_range, view, opts)?,
        view,
        opts,
    ))
}

/// Polar curve `r = f(θ)` for `θ` in `theta_range`, clipped to the view.
pub fn plot_polar<F>(
    f: &F,
    theta_range: (f64, f64),
    view: &View,
    opts: &PlotOptions,
) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let g = |th: f64| {
        let r = f(th);
        let (s, c) = th.sin_cos();
        (r * c, r * s)
    };
    plot_parametric(&g, theta_range, view, opts)
}

fn explicit_samples(span_px: f64, opts: &PlotOptions) -> usize {
    let n = (span_px / opts.initial_spacing_px.max(0.25)).ceil();
    if n.is_finite() {
        (n as usize).clamp(8, 100_000)
    } else {
        8
    }
}

/// Intersect `range` with `[lo, hi]`.
fn intersect(range: (f64, f64), lo: f64, hi: f64) -> Option<(f64, f64)> {
    let (a, b) = normalize_range(range.0, range.1)?;
    let (a, b) = (a.max(lo), b.min(hi));
    (a < b).then_some((a, b))
}

fn explicit_with<F>(
    f: &F,
    x_range: (f64, f64),
    view: &View,
    opts: &PlotOptions,
    budget: &mut Budget<'_>,
) -> Result<Vec<Polyline>, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let Some((a, b)) = normalize_range(x_range.0, x_range.1) else {
        return Ok(Vec::new());
    };
    let n = explicit_samples((b - a) / view.units_per_px_x(), opts);
    sample_with(&|x: f64| (x, f(x)), a, b, n, view, opts, budget)
}

/// Adaptive samples of `y = f(x)` for `x` in `x_range`, unclipped.
pub fn sample_explicit<F>(
    f: &F,
    x_range: (f64, f64),
    view: &View,
    opts: &PlotOptions,
) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let mut budget = Budget::new(opts.max_evals, &opts.cancel);
    let data = explicit_with(f, x_range, view, opts, &mut budget)?;
    Ok(Sampled {
        data,
        truncated: budget.truncated(),
    })
}

/// Adaptive samples of `x = f(y)` for `y` in `y_range`, unclipped.
pub fn sample_explicit_x<F>(
    f: &F,
    y_range: (f64, f64),
    view: &View,
    opts: &PlotOptions,
) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let mut budget = Budget::new(opts.max_evals, &opts.cancel);
    let Some((a, b)) = normalize_range(y_range.0, y_range.1) else {
        return Ok(Curves::default());
    };
    let n = explicit_samples((b - a) / view.units_per_px_y(), opts);
    let data = sample_with(&|y: f64| (f(y), y), a, b, n, view, opts, &mut budget)?;
    Ok(Sampled {
        data,
        truncated: budget.truncated(),
    })
}

/// The x range sampled for the view: the visible range plus one pixel.
pub(crate) fn view_x_range(view: &View) -> (f64, f64) {
    let r = view.bounds();
    let px = view.units_per_px_x();
    (r.x_min - px, r.x_max + px)
}

pub(crate) fn view_y_range(view: &View) -> (f64, f64) {
    let r = view.bounds();
    let px = view.units_per_px_y();
    (r.y_min - px, r.y_max + px)
}

/// `y = f(x)` across the view, clipped.
pub fn plot_explicit<F>(f: &F, view: &View, opts: &PlotOptions) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    Ok(clip_all(
        sample_explicit(f, view_x_range(view), view, opts)?,
        view,
        opts,
    ))
}

/// Several `y = f(x)` curves sharing one `max_evals` budget (e.g. a list of
/// curves `y = [1, 2, 3] x`). Each curve gets a fair share of what is left,
/// so a pathological curve cannot starve the ones after it.
pub fn plot_explicit_many<F>(
    fs: &[&F],
    view: &View,
    opts: &PlotOptions,
) -> Result<Vec<Curves>, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let mut budget = Budget::new(opts.max_evals, &opts.cancel);
    let total = budget.limit();
    let mut out = Vec::with_capacity(fs.len());
    for (i, f) in fs.iter().enumerate() {
        let share = budget.remaining() / (fs.len() - i);
        budget.set_limit(budget.evals + share);
        budget.reset_truncated();
        let data = explicit_with(*f, view_x_range(view), view, opts, &mut budget)?;
        let c = Sampled {
            data,
            truncated: budget.truncated(),
        };
        out.push(clip_all(c, view, opts));
        budget.set_limit(total);
    }
    Ok(out)
}

/// `y = f(x)` restricted to `x ∈ x_range` (e.g. `y = x² {0 < x < 2}`).
pub fn plot_explicit_on<F>(
    f: &F,
    x_range: (f64, f64),
    view: &View,
    opts: &PlotOptions,
) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let (lo, hi) = view_x_range(view);
    let Some(r) = intersect(x_range, lo, hi) else {
        return Ok(Curves::default());
    };
    Ok(clip_all(sample_explicit(f, r, view, opts)?, view, opts))
}

/// `x = f(y)` across the view, clipped.
pub fn plot_explicit_x<F>(f: &F, view: &View, opts: &PlotOptions) -> Result<Curves, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    Ok(clip_all(
        sample_explicit_x(f, view_y_range(view), view, opts)?,
        view,
        opts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancelToken;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn view() -> View {
        View::new(Rect::new(-10.0, -10.0, 10.0, 10.0), 1200.0, 800.0)
    }

    fn opts() -> PlotOptions {
        PlotOptions::default()
    }

    fn all_points(lines: &[Polyline]) -> impl Iterator<Item = Vec2> + '_ {
        lines.iter().flat_map(|l| l.points.iter().copied())
    }

    /// True if any segment of any polyline straddles `x = c`.
    fn crosses_x(lines: &[Polyline], c: f64) -> bool {
        lines.iter().any(|l| {
            l.points
                .windows(2)
                .any(|w| (w[0].x - c) * (w[1].x - c) < 0.0)
        })
    }

    #[test]
    fn sine_single_accurate_polyline() {
        let v = view();
        let lines = plot_explicit(&f64::sin, &v, &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        let pts = &lines[0].points;
        assert!(pts.first().unwrap().x <= -10.0 && pts.last().unwrap().x >= 10.0);
        for p in pts {
            assert!((p.y - p.x.sin()).abs() < 1e-12);
        }
        // Segment midpoints stay within the pixel tolerance of the curve.
        let upy = v.units_per_px_y();
        for w in pts.windows(2) {
            let mx = 0.5 * (w[0].x + w[1].x);
            let my = 0.5 * (w[0].y + w[1].y);
            assert!((my - mx.sin()).abs() / upy < 0.6);
        }
    }

    #[test]
    fn tan_breaks_at_asymptotes() {
        let lines = plot_explicit(&f64::tan, &view(), &opts()).unwrap();
        // Asymptotes in [-10, 10]: ±π/2, ±3π/2, ±5π/2 → 7 branches.
        assert_eq!(lines.len(), 7);
        for k in -3..3 {
            assert!(!crosses_x(&lines, FRAC_PI_2 + k as f64 * PI));
        }
        // Inner branches run from the bottom to the top of the clip area.
        let mid = &lines[3].points;
        assert!(mid.first().unwrap().y <= -10.0);
        assert!(mid.last().unwrap().y >= 10.0);
    }

    #[test]
    fn reciprocal_two_branches() {
        let lines = plot_explicit(&|x: f64| 1.0 / x, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(!crosses_x(&lines, 0.0));
        assert!(lines[0].points.iter().all(|p| p.x < 0.0));
        assert!(lines[1].points.iter().all(|p| p.x > 0.0));
        // Each branch reaches the clip edge near the asymptote.
        assert!(lines[0].points.last().unwrap().y < -10.0);
        assert!(lines[1].points[0].y > 10.0);
    }

    #[test]
    fn reciprocal_square_same_sign_pole() {
        let lines = plot_explicit(&|x: f64| 1.0 / (x * x), &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(!crosses_x(&lines, 0.0));
    }

    #[test]
    fn floor_has_no_vertical_risers() {
        let lines = plot_explicit(&f64::floor, &view(), &opts()).unwrap();
        // One horizontal step per unit interval in [-10, 10] (+ edge pixel).
        assert!((20..=22).contains(&lines.len()), "{}", lines.len());
        for l in &lines {
            let y0 = l.points[0].y;
            assert!(l.points.iter().all(|p| p.y == y0));
        }
    }

    #[test]
    fn sign_square_wave_breaks() {
        let f = |x: f64| if x.sin() >= 0.0 { 1.0 } else { -1.0 };
        let lines = plot_explicit(&f, &view(), &opts()).unwrap();
        // Jumps at kπ for k in -3..=3 → 8 pieces.
        assert_eq!(lines.len(), 8);
    }

    #[test]
    fn sqrt_reaches_domain_boundary() {
        let lines = plot_explicit(&f64::sqrt, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        let first = lines[0].points[0];
        assert!(first.x >= 0.0 && first.x < 1e-12, "{first:?}");
        assert!(first.y < 1e-5);
    }

    #[test]
    fn semicircle_reaches_both_endpoints() {
        let f = |x: f64| (1.0 - x * x).sqrt();
        let lines = plot_explicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        let pts = &lines[0].points;
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        assert!((a.x + 1.0).abs() < 1e-9 && a.y < 1e-4, "{a:?}");
        assert!((b.x - 1.0).abs() < 1e-9 && b.y < 1e-4, "{b:?}");
    }

    #[test]
    fn ln_dives_to_clip_edge() {
        let lines = plot_explicit(&f64::ln, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        let first = lines[0].points[0];
        assert!(first.x > 0.0 && first.y <= -12.0, "{first:?}");
        assert!(all_points(&lines).all(|p| p.is_finite()));
    }

    #[test]
    fn removable_singularity_is_bridged() {
        let f = |x: f64| x.sin() / x;
        // Symmetric view with even sample count hits x = 0 exactly.
        let lines = plot_explicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn nowhere_defined_is_empty() {
        let lines = plot_explicit(&|_x: f64| f64::NAN, &view(), &opts()).unwrap();
        assert!(lines.is_empty());
        let lines = plot_explicit(&|x: f64| (-1.0 - x * x).sqrt(), &view(), &opts()).unwrap();
        assert!(lines.is_empty());
    }

    #[test]
    fn offscreen_function_is_empty() {
        let lines = plot_explicit(&|x: f64| x + 1000.0, &view(), &opts()).unwrap();
        assert!(lines.is_empty());
    }

    #[test]
    fn steep_continuous_is_not_broken() {
        let lines = plot_explicit(&|x: f64| (1000.0 * x).tanh() * 5.0, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        let lines = plot_explicit(&f64::cbrt, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn restricted_domain() {
        let f = |x: f64| x * x;
        let lines = plot_explicit_on(&f, (0.0, 2.0), &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(all_points(&lines).all(|p| (0.0..=2.0).contains(&p.x)));
        assert!(
            plot_explicit_on(&f, (50.0, 60.0), &view(), &opts())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn explicit_in_y() {
        let lines = plot_explicit_x(&|y: f64| y * y - 3.0, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        // Points inserted on the clip boundary are interpolated; skip them.
        for p in all_points(&lines).filter(|p| p.x.abs() < 14.9) {
            assert!((p.x - (p.y * p.y - 3.0)).abs() < 1e-9);
        }
        let lines = plot_explicit_x(&|y: f64| 1.0 / y, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn far_offset_view() {
        let v = View::new(Rect::new(1e6 - 1.0, -2.0, 1e6 + 1.0, 2.0), 800.0, 600.0);
        let lines = plot_explicit(&|x: f64| (x - 1e6).sin(), &v, &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].points.len() > 100);
    }

    #[test]
    fn parametric_circle_is_closed() {
        let v = View::new(Rect::new(-2.0, -2.0, 2.0, 2.0), 800.0, 800.0);
        let lines = plot_parametric(&|t: f64| (t.cos(), t.sin()), (0.0, TAU), &v, &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        for p in &lines[0].points {
            assert!((p.length() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn parametric_reversed_range_and_empty() {
        let v = view();
        let f = |t: f64| (t, 2.0 * t);
        assert_eq!(
            plot_parametric(&f, (3.0, -3.0), &v, &opts()).unwrap().len(),
            1
        );
        assert!(
            plot_parametric(&f, (1.0, 1.0), &v, &opts())
                .unwrap()
                .is_empty()
        );
        assert!(
            plot_parametric(&f, (0.0, f64::INFINITY), &v, &opts())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parametric_hyperbola_gap() {
        let f = |t: f64| (t, 1.0 / t);
        let lines = plot_parametric(&f, (-5.0, 5.0), &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn polar_cardioid() {
        let v = View::new(Rect::new(-3.0, -3.0, 3.0, 3.0), 600.0, 600.0);
        let lines = plot_polar(&|th: f64| 1.0 + th.cos(), (0.0, TAU), &v, &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        for p in &lines[0].points {
            let th = p.y.atan2(p.x);
            assert!((p.length() - (1.0 + th.cos())).abs() < 1e-9);
        }
    }

    #[test]
    fn polar_rose_and_spiral() {
        let v = View::new(Rect::new(-2.0, -2.0, 2.0, 2.0), 600.0, 600.0);
        let rose = plot_polar(&|th: f64| (3.0 * th).cos(), (0.0, PI), &v, &opts()).unwrap();
        assert_eq!(rose.len(), 1);
        let v = View::new(Rect::new(-40.0, -40.0, 40.0, 40.0), 600.0, 600.0);
        let spiral = plot_polar(&|th: f64| th, (0.0, 10.0 * PI), &v, &opts()).unwrap();
        assert_eq!(spiral.len(), 1);
        assert!(!spiral[0].closed);
    }

    #[test]
    fn cancelled_returns_error() {
        let o = PlotOptions::with_cancel(CancelToken::new());
        o.cancel.cancel();
        assert_eq!(plot_explicit(&f64::sin, &view(), &o), Err(Cancelled));
    }

    #[test]
    fn eval_budget_bounds_pathological_input() {
        let count = AtomicUsize::new(0);
        let f = |x: f64| {
            count.fetch_add(1, Ordering::Relaxed);
            (1.0 / x).sin()
        };
        let o = PlotOptions {
            max_evals: 20_000,
            ..opts()
        };
        let v = View::new(Rect::new(-1.0, -1.5, 1.0, 1.5), 1200.0, 800.0);
        let lines = plot_explicit(&f, &v, &o).unwrap();
        assert!(!lines.is_empty());
        assert!(lines.truncated);
        // A hard cap: never a single evaluation more.
        assert!(count.load(Ordering::Relaxed) <= 20_000);
    }

    #[test]
    fn tiny_budget_is_a_hard_cap_including_initial_pass() {
        for max in [1, 2, 10, 100, 1000] {
            let count = AtomicUsize::new(0);
            let f = |x: f64| {
                count.fetch_add(1, Ordering::Relaxed);
                x.tan()
            };
            let o = PlotOptions {
                max_evals: max,
                ..opts()
            };
            let c = plot_explicit(&f, &view(), &o).unwrap();
            assert!(count.load(Ordering::Relaxed) <= max, "{max}");
            assert!(c.truncated);
            let count = AtomicUsize::new(0);
            let g = |t: f64| {
                count.fetch_add(1, Ordering::Relaxed);
                (t.cos(), t.sin())
            };
            let o = PlotOptions {
                max_evals: max,
                parametric_samples: usize::MAX,
                ..opts()
            };
            plot_parametric(&g, (0.0, TAU), &view(), &o).unwrap();
            assert!(count.load(Ordering::Relaxed) <= max, "{max}");
        }
        assert!(
            !plot_explicit(&f64::sin, &view(), &opts())
                .unwrap()
                .truncated
        );
    }

    #[test]
    fn narrow_excursion_into_view_is_found() {
        // Baseline far above the view with a ~2 px wide dip down to y = -5,
        // centered a quarter of the way into an initial interval: the
        // samples a, m (and b) of that interval are all above the view, so
        // culling at depth 0 would skip the dip entirely.
        let v = view();
        let upp = v.units_per_px_x();
        let n = ((20.0 + 2.0 * upp) / upp / 3.0).ceil();
        let h = (20.0 + 2.0 * upp) / n;
        let c0 = -10.0 - upp + 200.0 * h + 0.25 * h;
        let f = move |x: f64| 50.0 - 55.0 * (-((x - c0) / 0.015).powi(2)).exp();
        assert!(f(c0 - 0.25 * h) > 12.0 && f(c0 + 0.25 * h) > 12.0);
        let c = plot_explicit(&f, &view(), &opts()).unwrap();
        assert!(!c.is_empty());
        let min_y = c
            .iter()
            .flat_map(|l| l.points.iter())
            .fold(f64::INFINITY, |m, p| m.min(p.y));
        assert!(min_y < -4.9, "{min_y}");
    }

    #[test]
    fn many_curves_share_one_budget() {
        let count = AtomicUsize::new(0);
        let wild = |x: f64| {
            count.fetch_add(1, Ordering::Relaxed);
            (1.0 / x).sin()
        };
        let tame = |x: f64| {
            count.fetch_add(1, Ordering::Relaxed);
            x.sin()
        };
        let fs: [&dyn Fn(f64) -> f64; 2] = [&wild, &tame];
        let o = PlotOptions {
            max_evals: 6_000,
            ..opts()
        };
        let v = View::new(Rect::new(-1.0, -1.5, 1.0, 1.5), 1200.0, 800.0);
        let out = plot_explicit_many(&fs, &v, &o).unwrap();
        assert_eq!(out.len(), 2);
        assert!(count.load(Ordering::Relaxed) <= 6_000);
        assert!(out[0].truncated);
        // The tame curve still gets its share and completes.
        assert!(!out[1].truncated);
        assert_eq!(out[1].len(), 1);
    }

    #[test]
    fn runs_on_worker_thread() {
        let f: Arc<dyn Fn(f64) -> f64 + Send + Sync> = Arc::new(|x: f64| x.cos());
        let token = CancelToken::new();
        let o = PlotOptions::with_cancel(token.clone());
        let handle = std::thread::spawn(move || plot_explicit(&*f, &view(), &o));
        let lines = handle.join().unwrap().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(!token.is_cancelled());
    }
}
