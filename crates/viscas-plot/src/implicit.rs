//! Implicit curves `F(x, y) = 0` by adaptive marching squares.
//!
//! Algorithm:
//! 1. A coarse grid (`implicit_cell_px`) aligned to world multiples of the
//!    cell size, so panning does not make contours shimmer. The coarse grid
//!    is enlarged up front if it would not fit in `max_evals`.
//! 2. Cells whose corners change sign are refined as a quadtree down to
//!    `implicit_depth` levels, one whole level at a time. A level is only
//!    started if the budget can pay for all of it, so budget exhaustion never
//!    leaves mixed-depth neighbours (T-junctions) along the curve. Corner
//!    values are cached per vertex.
//! 3. Coarse cells whose corners all have the same sign are probed at their
//!    center; a sign change there refines the cell too, which catches small
//!    closed loops and thin features about one cell wide.
//! 4. On each refined cell edge with a sign change, the crossing is located
//!    with Illinois (modified regula falsi) iterations down to a tight
//!    bracket. If `|F|` there is not small relative to the edge endpoint
//!    values, the sign change is a pole or a jump (e.g. `1/(x y) = 1` across
//!    the axes, `y = tan x` at the asymptotes) and no contour is produced.
//! 5. Saddle cells (four crossings) are disambiguated with the center value.
//! 6. Segments share crossing points by edge key, so they are stitched into
//!    polylines by walking edge adjacency; loops come out `closed`.
//!
//! Remaining limits: features that never change sign at a sampled vertex or
//! coarse-cell center are missed. That covers loops much smaller than one
//! coarse cell away from its center, and zero sets where `F` touches 0
//! without crossing it (`(x² + y² - 1)² = 0`).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::cancel::{Budget, Cancelled, Sampled};
use crate::curve::Curves;
use crate::geom::{Polyline, Vec2};
use crate::options::PlotOptions;
use crate::view::View;

/// Small multiplicative hasher for integer grid keys (std SipHash is slow
/// for the many tiny lookups marching squares does).
#[derive(Default)]
pub(crate) struct GridHasher(u64);

impl Hasher for GridHasher {
    fn finish(&self) -> u64 {
        // splitmix64 finalizer: spreads high-bit differences into the low
        // bits that hashbrown uses for bucket selection.
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }

    fn write_u8(&mut self, v: u8) {
        self.write_u64(u64::from(v));
    }
}

pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<GridHasher>>;

/// Grid vertex in finest-level units.
type Key = (i64, i64);
/// Edge: start vertex, direction (0 = +x, 1 = +y), length in finest units.
pub(crate) type EdgeKey = (i64, i64, u8, i64);

/// Result of looking for a zero on a cell edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Crossing {
    /// No sign change between finite endpoint values.
    None,
    /// A genuine root of `F`.
    Root(Vec2),
    /// Sign change through a pole or jump (linear-interpolated position).
    Pole(Vec2),
}

impl Crossing {
    pub fn point(self) -> Option<Vec2> {
        match self {
            Crossing::None => None,
            Crossing::Root(p) | Crossing::Pole(p) => Some(p),
        }
    }

    pub fn root(self) -> Option<Vec2> {
        match self {
            Crossing::Root(p) => Some(p),
            _ => None,
        }
    }
}

/// Square cell: lower-left vertex and size, in finest units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub i: i64,
    pub j: i64,
    pub size: i64,
}

impl Cell {
    /// Corners counter-clockwise from lower-left.
    pub fn corners(self) -> [Key; 4] {
        let (i, j, s) = (self.i, self.j, self.size);
        [(i, j), (i + s, j), (i + s, j + s), (i, j + s)]
    }

    /// Edges matching `corners()`: edge `k` joins corner `k` and `k + 1`.
    pub fn edges(self) -> [EdgeKey; 4] {
        let (i, j, s) = (self.i, self.j, self.size);
        [
            (i, j, 0, s),
            (i + s, j, 1, s),
            (i, j + s, 0, s),
            (i, j, 1, s),
        ]
    }

    fn children(self) -> [Cell; 4] {
        let h = self.size / 2;
        [(0, 0), (h, 0), (0, h), (h, h)].map(|(di, dj)| Cell {
            i: self.i + di,
            j: self.j + dj,
            size: h,
        })
    }
}

/// Inside means `F < 0` (finite).
#[inline]
pub(crate) fn inside(v: f64) -> bool {
    v < 0.0
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// All four corners inside.
    Full,
    /// Inside corners together with outside or undefined ones.
    Mixed,
    /// All finite with the same sign (outside, or inside handled as Full).
    Uniform,
    /// Nothing inside: outside and/or undefined corners.
    Empty,
}

/// Evaluations a coarse cell costs: its share of vertices plus one probe.
const COARSE_COST: usize = 2;
/// New vertices when one cell is split into four.
const SPLIT_COST: usize = 5;
/// Budget kept per mixed cell for edge roots of the next level.
const CONTOUR_RESERVE: usize = 16;
/// Illinois iteration cap per edge.
const EDGE_ITERS: usize = 60;

pub(crate) struct Grid<'a, F: ?Sized> {
    f: &'a F,
    cw: f64,
    ch: f64,
    values: FastMap<Key, f64>,
    edges: FastMap<EdgeKey, Crossing>,
    budget: Budget<'a>,
    /// The coarse grid was enlarged or a refinement level was skipped.
    coarsened: bool,
    /// Cells with all four corners inside (any quadtree level).
    pub full: Vec<Cell>,
    /// Leaf cells with mixed corners (all at the same quadtree level).
    pub mixed: Vec<Cell>,
}

impl<'a, F> Grid<'a, F>
where
    F: Fn(f64, f64) -> f64 + ?Sized,
{
    /// Evaluate the adaptive grid over the view.
    pub fn build(f: &'a F, view: &View, opts: &'a PlotOptions) -> Result<Self, Cancelled> {
        let depth = opts.implicit_depth.min(8);
        let step: i64 = 1 << depth;
        let b = view.bounds();
        let mut cell_px = opts.implicit_cell_px.max(1.0);
        let mut coarsened = false;
        // Size the coarse grid so that it costs at most half the budget.
        let (ci0, ci1, cj0, cj1, cwc, chc) = loop {
            let cwc = cell_px * view.units_per_px_x();
            let chc = cell_px * view.units_per_px_y();
            let ci0 = (b.x_min / cwc).floor() as i64;
            let ci1 = (b.x_max / cwc).ceil() as i64;
            let cj0 = (b.y_min / chc).floor() as i64;
            let cj1 = (b.y_max / chc).ceil() as i64;
            let cells = ((ci1 - ci0).max(1) * (cj1 - cj0).max(1)) as usize;
            let cost = cells * COARSE_COST + 2 * ((ci1 - ci0) + (cj1 - cj0)) as usize + 1;
            if cost <= opts.max_evals / 2 || cells == 1 {
                break (ci0, ci1, cj0, cj1, cwc, chc);
            }
            cell_px *= 1.5;
            coarsened = true;
        };
        let ncells = ((ci1 - ci0) * (cj1 - cj0)).max(0) as usize;
        let mut g = Grid {
            f,
            cw: cwc / step as f64,
            ch: chc / step as f64,
            values: FastMap::with_capacity_and_hasher(ncells * 2, Default::default()),
            edges: FastMap::default(),
            budget: Budget::new(opts.max_evals, &opts.cancel),
            coarsened,
            full: Vec::new(),
            mixed: Vec::new(),
        };
        let mut level = Vec::new();
        for cj in cj0..cj1 {
            for ci in ci0..ci1 {
                let c = Cell {
                    i: ci * step,
                    j: cj * step,
                    size: step,
                };
                match g.classify(c)? {
                    Class::Full => g.full.push(c),
                    Class::Mixed => level.push(c),
                    Class::Uniform => {
                        // Probe the center for features inside the cell.
                        let corner = g.value(c.corners()[0])?;
                        if let Some(vc) = g.center_value(c)?
                            && vc.is_finite()
                            && inside(vc) != inside(corner)
                        {
                            level.push(c);
                        }
                    }
                    Class::Empty => {}
                }
            }
        }
        // Refine whole levels while the budget can pay for them.
        let mut size = step;
        while size > 1 && !level.is_empty() {
            if g.budget.remaining() < level.len() * (SPLIT_COST + CONTOUR_RESERVE) {
                g.coarsened = true;
                break;
            }
            let mut next = Vec::with_capacity(level.len() * 2);
            for c in level {
                for child in c.children() {
                    match g.classify(child)? {
                        Class::Full => g.full.push(child),
                        Class::Mixed => next.push(child),
                        Class::Uniform | Class::Empty => {}
                    }
                }
            }
            level = next;
            size /= 2;
        }
        g.mixed = level;
        Ok(g)
    }

    fn classify(&mut self, c: Cell) -> Result<Class, Cancelled> {
        let mut neg = 0;
        let mut pos = 0;
        for k in c.corners() {
            let v = self.value(k)?;
            if inside(v) {
                neg += 1;
            } else if v.is_finite() {
                pos += 1;
            }
        }
        Ok(match (neg, pos) {
            (4, _) => Class::Full,
            (0, 4) => Class::Uniform,
            (0, _) => Class::Empty,
            _ => Class::Mixed,
        })
    }

    /// The budget ran out or forced a coarser grid.
    pub fn truncated(&self) -> bool {
        self.coarsened || self.budget.truncated()
    }

    pub fn pos(&self, k: Key) -> Vec2 {
        Vec2::new(k.0 as f64 * self.cw, k.1 as f64 * self.ch)
    }

    /// Evaluate `F` (non-finite mapped to NaN), or `None` without budget.
    fn eval(&mut self, p: Vec2) -> Result<Option<f64>, Cancelled> {
        if !self.budget.take()? {
            return Ok(None);
        }
        let v = (self.f)(p.x, p.y);
        Ok(Some(if v.is_finite() { v } else { f64::NAN }))
    }

    /// Cached vertex value. The level-wise budgeting means this is always
    /// granted in practice; if not, the vertex is recorded as undefined.
    pub fn value(&mut self, k: Key) -> Result<f64, Cancelled> {
        if let Some(&v) = self.values.get(&k) {
            return Ok(v);
        }
        let v = self.eval(self.pos(k))?.unwrap_or(f64::NAN);
        self.values.insert(k, v);
        Ok(v)
    }

    /// Value at the center of a cell (not cached), `None` without budget.
    pub fn center_value(&mut self, c: Cell) -> Result<Option<f64>, Cancelled> {
        let h = 0.5 * c.size as f64;
        let p = Vec2::new((c.i as f64 + h) * self.cw, (c.j as f64 + h) * self.ch);
        self.eval(p)
    }

    /// Center value for saddle resolution, falling back to the corner mean.
    pub fn saddle_center(&mut self, c: Cell) -> Result<f64, Cancelled> {
        if let Some(v) = self.center_value(c)? {
            return Ok(v);
        }
        let mut s = 0.0;
        for k in c.corners() {
            s += self.value(k)?;
        }
        Ok(0.25 * s)
    }

    pub fn crossing(&mut self, e: EdgeKey) -> Result<Crossing, Cancelled> {
        if let Some(&c) = self.edges.get(&e) {
            return Ok(c);
        }
        let (i, j, dir, s) = e;
        let ka = (i, j);
        let kb = if dir == 0 { (i + s, j) } else { (i, j + s) };
        let (va, vb) = (self.value(ka)?, self.value(kb)?);
        let c = if va.is_finite() && vb.is_finite() && inside(va) != inside(vb) {
            self.edge_root(self.pos(ka), va, self.pos(kb), vb)?
        } else {
            Crossing::None
        };
        self.edges.insert(e, c);
        Ok(c)
    }

    /// Illinois iterations on the edge down to a tight bracket, then reject
    /// poles/jumps by checking that `|F|` actually became small. Without
    /// budget the linear interpolant is used unverified (result truncated).
    fn edge_root(&mut self, a: Vec2, va: f64, b: Vec2, vb: f64) -> Result<Crossing, Cancelled> {
        let lin = a.lerp(b, va / (va - vb));
        let scale = va.abs().max(vb.abs());
        // Bracket [lo, hi] with true values (glo, ghi) and Illinois-scaled
        // values (flo, fhi).
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let (mut glo, mut ghi) = (va, vb);
        let (mut flo, mut fhi) = (va, vb);
        let mut side = 0i8;
        let mut starved = false;
        for _ in 0..EDGE_ITERS {
            if hi - lo <= 1e-12 {
                break;
            }
            let mut t = (lo * fhi - hi * flo) / (fhi - flo);
            if !(t > lo && t < hi) {
                t = 0.5 * (lo + hi);
            }
            let Some(ft) = self.eval(a.lerp(b, t))? else {
                starved = true;
                break;
            };
            if !ft.is_finite() {
                return Ok(Crossing::Pole(lin));
            }
            if ft == 0.0 {
                return Ok(Crossing::Root(a.lerp(b, t)));
            }
            if inside(ft) == inside(glo) {
                lo = t;
                glo = ft;
                flo = ft;
                if side == -1 {
                    fhi *= 0.5;
                }
                side = -1;
            } else {
                hi = t;
                ghi = ft;
                fhi = ft;
                if side == 1 {
                    flo *= 0.5;
                }
                side = 1;
            }
        }
        let (t, best) = if glo.abs() <= ghi.abs() {
            (lo, glo)
        } else {
            (hi, ghi)
        };
        if starved {
            // Cannot verify; keep the best estimate (result is truncated).
            return Ok(Crossing::Root(if lo == 0.0 && hi == 1.0 {
                lin
            } else {
                a.lerp(b, t)
            }));
        }
        // A true root drives |F| to ~0 relative to the endpoint values; at a
        // jump it stays at the jump height, at a pole it blows up.
        if best.abs() <= 1e-6 * scale {
            Ok(Crossing::Root(a.lerp(b, t)))
        } else {
            Ok(Crossing::Pole(lin))
        }
    }

    /// Contour segments of a mixed cell as pairs of edge keys.
    pub fn cell_segments(
        &mut self,
        c: Cell,
        out: &mut Vec<(EdgeKey, EdgeKey)>,
    ) -> Result<(), Cancelled> {
        let edges = c.edges();
        let mut cross = [Crossing::None; 4];
        for k in 0..4 {
            cross[k] = self.crossing(edges[k])?;
        }
        let changing: Vec<usize> = (0..4).filter(|&k| cross[k] != Crossing::None).collect();
        let mut push = |a: usize, b: usize| {
            if cross[a].root().is_some() && cross[b].root().is_some() {
                out.push((edges[a], edges[b]));
            }
        };
        match changing.len() {
            2 => push(changing[0], changing[1]),
            4 => {
                let v00 = self.value(c.corners()[0])?;
                let vc = self.saddle_center(c)?;
                if inside(vc) == inside(v00) {
                    // v00 and v11 are joined through the center.
                    push(0, 1);
                    push(2, 3);
                } else {
                    push(0, 3);
                    push(1, 2);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn contour(&mut self) -> Result<Vec<Polyline>, Cancelled> {
        let mut segs = Vec::new();
        for c in self.mixed.clone() {
            self.cell_segments(c, &mut segs)?;
        }
        Ok(stitch(&segs, |e| {
            self.edges
                .get(&e)
                .and_then(|c| c.root())
                .unwrap_or_default()
        }))
    }
}

/// Join segments sharing edge keys into polylines.
pub(crate) fn stitch(segs: &[(EdgeKey, EdgeKey)], pos: impl Fn(EdgeKey) -> Vec2) -> Vec<Polyline> {
    const NONE: usize = usize::MAX;
    let mut adj: FastMap<EdgeKey, [usize; 2]> = FastMap::default();
    for (idx, &(a, b)) in segs.iter().enumerate() {
        for e in [a, b] {
            let slot = adj.entry(e).or_insert([NONE, NONE]);
            if slot[0] == NONE {
                slot[0] = idx;
            } else {
                slot[1] = idx;
            }
        }
    }
    let mut used = vec![false; segs.len()];
    let next = |used: &[bool], at: EdgeKey| -> Option<usize> {
        adj.get(&at)?
            .iter()
            .copied()
            .find(|&s| s != NONE && !used[s])
    };
    let mut out = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (a, b) = segs[start];
        let mut fwd = vec![a, b];
        let mut closed = false;
        let mut end = b;
        while let Some(s) = next(&used, end) {
            used[s] = true;
            let (p, q) = segs[s];
            end = if p == end { q } else { p };
            if end == a {
                closed = true;
                break;
            }
            fwd.push(end);
        }
        if !closed {
            let mut back = Vec::new();
            let mut end = a;
            while let Some(s) = next(&used, end) {
                used[s] = true;
                let (p, q) = segs[s];
                end = if p == end { q } else { p };
                back.push(end);
            }
            back.reverse();
            back.extend(fwd);
            fwd = back;
        }
        out.push(Polyline {
            points: fwd.into_iter().map(&pos).collect(),
            closed,
        });
    }
    out
}

/// Implicit curve `F(x, y) = 0` over the view.
pub fn plot_implicit<F>(f: &F, view: &View, opts: &PlotOptions) -> Result<Curves, Cancelled>
where
    F: Fn(f64, f64) -> f64 + ?Sized,
{
    // Crossings are where the sign flips, so `F < 0` vs `F >= 0` is enough.
    let mut g = Grid::build(f, view, opts)?;
    let data = g.contour()?;
    Ok(Sampled {
        data,
        truncated: g.truncated(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancelToken, Rect};

    fn view() -> View {
        View::new(Rect::new(-3.0, -2.0, 3.0, 2.0), 1200.0, 800.0)
    }

    fn opts() -> PlotOptions {
        PlotOptions::default()
    }

    fn check_on_curve(lines: &[Polyline], f: impl Fn(f64, f64) -> f64, tol: f64) {
        for l in lines {
            for p in &l.points {
                let v = f(p.x, p.y);
                assert!(v.abs() < tol, "F({}, {}) = {v}", p.x, p.y);
            }
        }
    }

    #[test]
    fn circle_is_one_closed_loop() {
        let f = |x: f64, y: f64| x * x + y * y - 1.0;
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        assert!(!lines.truncated);
        assert!(lines[0].points.len() > 500);
        // Edge roots are iterated to a tight bracket.
        check_on_curve(&lines, f, 1e-12);
    }

    #[test]
    fn budget_exhaustion_keeps_loops_closed() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for max in [500, 3_000, 12_000, 25_000, 40_000] {
            let n = AtomicUsize::new(0);
            let f = |x: f64, y: f64| {
                n.fetch_add(1, Ordering::Relaxed);
                x * x + y * y - 1.0
            };
            let o = PlotOptions {
                max_evals: max,
                ..opts()
            };
            let lines = plot_implicit(&f, &view(), &o).unwrap();
            assert!(
                n.load(Ordering::Relaxed) <= max,
                "{max}: {}",
                n.load(Ordering::Relaxed)
            );
            assert!(lines.truncated, "{max}");
            assert_eq!(lines.len(), 1, "{max}");
            assert!(lines[0].closed, "{max}");
        }
    }

    #[test]
    fn center_probe_finds_small_loop() {
        let v = view();
        let cell = 8.0 * v.units_per_px_x();
        // Radius 2.5 px circle centered in a coarse cell: all corners outside.
        let (cx, cy) = (2.5 * cell, 1.5 * cell);
        let r = 2.5 * v.units_per_px_x();
        let f = |x: f64, y: f64| (x - cx).powi(2) + (y - cy).powi(2) - r * r;
        let lines = plot_implicit(&f, &v, &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
    }

    #[test]
    fn grid_hasher_spreads_high_bits() {
        use std::collections::HashSet;
        use std::hash::BuildHasher;
        let b = BuildHasherDefault::<GridHasher>::default();
        let low: HashSet<u64> = (0..1000i64)
            .map(|k| b.hash_one((k << 40, 7i64)) & 0xffff)
            .collect();
        assert!(low.len() > 950, "{}", low.len());
    }

    #[test]
    fn hyperbola_two_open_branches() {
        let f = |x: f64, y: f64| x * x - y * y - 1.0;
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| !l.closed));
        assert!(lines.iter().any(|l| l.points.iter().all(|p| p.x > 0.0)));
        assert!(lines.iter().any(|l| l.points.iter().all(|p| p.x < 0.0)));
        check_on_curve(&lines, f, 1e-5);
    }

    #[test]
    fn reciprocal_product_has_no_pole_contours() {
        // 1/(xy) = 1 changes sign across both axes through poles.
        let f = |x: f64, y: f64| 1.0 / (x * y) - 1.0;
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 2);
        check_on_curve(&lines, |x, y| x * y - 1.0, 1e-9);
    }

    #[test]
    fn implicit_tan_rejects_asymptotes() {
        let f = |x: f64, y: f64| y - x.tan();
        let v = View::new(Rect::new(-5.0, -3.0, 5.0, 3.0), 1000.0, 600.0);
        let lines = plot_implicit(&f, &v, &opts()).unwrap();
        // Branches between asymptotes at ±π/2, ±3π/2 → 5 pieces.
        assert_eq!(lines.len(), 5);
        check_on_curve(&lines, f, 1e-3);
    }

    #[test]
    fn floor_jumps_are_not_contours() {
        let f = |x: f64, y: f64| y - x.floor() - 0.5;
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        check_on_curve(&lines, f, 1e-6);
        // Visible steps y = -1.5, -0.5, 0.5, 1.5.
        assert_eq!(lines.len(), 4);
        for l in &lines {
            let y0 = l.points[0].y;
            assert!(l.points.iter().all(|p| (p.y - y0).abs() < 1e-9));
        }
    }

    #[test]
    fn two_disjoint_circles() {
        let f = |x: f64, y: f64| {
            let a = (x - 1.5).powi(2) + y * y - 0.25;
            let b = (x + 1.5).powi(2) + y * y - 0.25;
            a.min(b)
        };
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.closed));
    }

    #[test]
    fn lemniscate_saddle() {
        let f = |x: f64, y: f64| (x * x + y * y).powi(2) - 2.0 * (x * x - y * y);
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        assert!(!lines.is_empty());
        check_on_curve(&lines, f, 1e-4);
        let n: usize = lines.iter().map(|l| l.len()).sum();
        assert!(n > 400);
    }

    #[test]
    fn line_through_grid_vertices() {
        let f = |x: f64, y: f64| x - y;
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        check_on_curve(&lines, f, 1e-9);
    }

    #[test]
    fn no_zero_set_is_empty() {
        let f = |x: f64, y: f64| x * x + y * y + 1.0;
        assert!(plot_implicit(&f, &view(), &opts()).unwrap().is_empty());
    }

    #[test]
    fn partial_domain() {
        let f = |x: f64, y: f64| y - x.sqrt();
        let lines = plot_implicit(&f, &view(), &opts()).unwrap();
        assert_eq!(lines.len(), 1);
        check_on_curve(&lines, f, 1e-4);
    }

    #[test]
    fn grid_is_world_aligned() {
        let f = |x: f64, y: f64| x * x + y * y - 1.0;
        let v = view();
        // Shift by exactly one coarse cell (8 px).
        let cell = 8.0 * v.units_per_px_x();
        let mut v2 = v;
        v2.pan_world(cell, 0.0);
        let a = plot_implicit(&f, &v, &opts()).unwrap();
        let b = plot_implicit(&f, &v2, &opts()).unwrap();
        let mut pa: Vec<_> = a[0].points.iter().map(|p| (p.x, p.y)).collect();
        let mut pb: Vec<_> = b[0].points.iter().map(|p| (p.x, p.y)).collect();
        pa.sort_by(|x, y| x.partial_cmp(y).unwrap());
        pb.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert_eq!(pa.len(), pb.len());
        for (p, q) in pa.iter().zip(&pb) {
            assert!((p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9);
        }
    }

    #[test]
    fn cancel_aborts() {
        let o = PlotOptions::with_cancel(CancelToken::new());
        o.cancel.cancel();
        let f = |x: f64, y: f64| x * x + y * y - 1.0;
        assert_eq!(plot_implicit(&f, &view(), &o), Err(Cancelled));
    }

    #[test]
    fn stitch_joins_out_of_order_segments() {
        let e = |n: i64| (n, 0, 0, 1);
        let segs = vec![(e(2), e(3)), (e(0), e(1)), (e(1), e(2))];
        let lines = stitch(&segs, |k| Vec2::new(k.0 as f64, 0.0));
        assert_eq!(lines.len(), 1);
        let xs: Vec<f64> = lines[0].points.iter().map(|p| p.x).collect();
        assert!(xs == vec![0.0, 1.0, 2.0, 3.0] || xs == vec![3.0, 2.0, 1.0, 0.0]);
        let ring = vec![(e(0), e(1)), (e(1), e(2)), (e(2), e(0))];
        let lines = stitch(&ring, |k| Vec2::new(k.0 as f64, 0.0));
        assert!(lines[0].closed);
        assert_eq!(lines[0].points.len(), 3);
    }
}
