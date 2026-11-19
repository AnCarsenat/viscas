//! Shaded inequalities: `y ⋚ f(x)`, `x ⋚ f(y)` and `F(x, y) ⋚ 0`.
//!
//! Explicit inequalities fill between the adaptively sampled curve and the
//! clip band edge (one polygon per continuous piece, so domain gaps stay
//! unshaded). Implicit inequalities reuse the marching-squares quadtree:
//! fully-inside cells become rectangles (merged into row runs) and boundary
//! cells become the inside part of the cell clipped at the edge crossings.
//!
//! Fill polygons are clipped to the view bounds, so [`Region::area`] is the
//! visible shaded area. They never overlap; the UI should add them all to a
//! single path and fill once to avoid anti-aliasing seams between pieces.
//!
//! Sign changes through poles (e.g. `1/x < 0`) do bound the shaded region:
//! the fill closes at the asymptote, but no boundary curve is drawn there.
//! Boundary cells with an undefined (NaN) corner are left unshaded rather
//! than guessed, so domain edges may show a gap of up to one finest cell.

use crate::cancel::Cancelled;
use crate::curve::{sample_explicit, sample_explicit_x, view_x_range, view_y_range};
use crate::geom::{
    LineStyle, Polygon, Polyline, Primitive, Vec2, clamp_polyline_x, clamp_polyline_y,
};
use crate::implicit::{Cell, Grid, inside};
use crate::options::PlotOptions;
use crate::view::View;

/// Comparison operator of an inequality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Less,
    LessEq,
    Greater,
    GreaterEq,
}

impl Comparison {
    /// Strict inequalities draw a dashed boundary.
    pub fn is_strict(self) -> bool {
        matches!(self, Comparison::Less | Comparison::Greater)
    }

    pub fn is_less(self) -> bool {
        matches!(self, Comparison::Less | Comparison::LessEq)
    }

    pub fn boundary_style(self) -> LineStyle {
        if self.is_strict() {
            LineStyle::Dashed
        } else {
            LineStyle::Solid
        }
    }
}

/// A shaded region with its boundary curve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub fill: Vec<Polygon>,
    pub boundary: Vec<Polyline>,
    pub boundary_style: LineStyle,
    /// The evaluation budget ran out; the region is coarser or incomplete.
    pub truncated: bool,
}

impl Region {
    /// Visible fill area (world units²); fills are clipped to the view.
    pub fn area(&self) -> f64 {
        self.fill.iter().map(Polygon::area).sum()
    }

    /// Fill polygons followed by the boundary polylines.
    pub fn primitives(&self) -> Vec<Primitive> {
        let fills = self.fill.iter().cloned().map(Primitive::Polygon);
        let lines = self
            .boundary
            .iter()
            .cloned()
            .map(|line| Primitive::Polyline {
                line,
                style: self.boundary_style,
            });
        fills.chain(lines).collect()
    }
}

fn band(lo: f64, hi: f64, margin: f64) -> (f64, f64) {
    let m = (hi - lo) * margin.max(0.01);
    (lo - m, hi + m)
}

/// `y < f(x)`, `y <= f(x)`, `y > f(x)` or `y >= f(x)` over the view.
pub fn plot_explicit_inequality<F>(
    f: &F,
    cmp: Comparison,
    view: &View,
    opts: &PlotOptions,
) -> Result<Region, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let raw = sample_explicit(f, view_x_range(view), view, opts)?;
    let b = view.bounds();
    let (lo, hi) = band(b.y_min, b.y_max, opts.clip_margin);
    let edge = if cmp.is_less() { lo } else { hi };
    let fill = raw
        .iter()
        .filter_map(|line| {
            let mut pts = clamp_polyline_y(&line.points, lo, hi);
            let (first, last) = (*pts.first()?, *pts.last()?);
            pts.push(Vec2::new(last.x, edge));
            pts.push(Vec2::new(first.x, edge));
            let poly = Polygon::new(pts);
            (poly.area() > 0.0).then_some(poly)
        })
        .collect();
    Ok(finish(fill, raw.data, raw.truncated, cmp, view, opts))
}

/// `x ⋚ f(y)` over the view (shaded to the left for `<`, right for `>`).
pub fn plot_explicit_x_inequality<F>(
    f: &F,
    cmp: Comparison,
    view: &View,
    opts: &PlotOptions,
) -> Result<Region, Cancelled>
where
    F: Fn(f64) -> f64 + ?Sized,
{
    let raw = sample_explicit_x(f, view_y_range(view), view, opts)?;
    let b = view.bounds();
    let (lo, hi) = band(b.x_min, b.x_max, opts.clip_margin);
    let edge = if cmp.is_less() { lo } else { hi };
    let fill = raw
        .iter()
        .filter_map(|line| {
            let mut pts = clamp_polyline_x(&line.points, lo, hi);
            let (first, last) = (*pts.first()?, *pts.last()?);
            pts.push(Vec2::new(edge, last.y));
            pts.push(Vec2::new(edge, first.y));
            let poly = Polygon::new(pts);
            (poly.area() > 0.0).then_some(poly)
        })
        .collect();
    Ok(finish(fill, raw.data, raw.truncated, cmp, view, opts))
}

fn clip_fill(fill: Vec<Polygon>, view: &View) -> Vec<Polygon> {
    let b = view.bounds();
    fill.into_iter().filter_map(|p| p.clipped(&b)).collect()
}

fn finish(
    fill: Vec<Polygon>,
    raw: Vec<Polyline>,
    truncated: bool,
    cmp: Comparison,
    view: &View,
    opts: &PlotOptions,
) -> Region {
    let rect = view.bounds().expanded(opts.clip_margin.max(0.0));
    Region {
        fill: clip_fill(fill, view),
        boundary: raw.iter().flat_map(|l| l.clipped(&rect)).collect(),
        boundary_style: cmp.boundary_style(),
        truncated,
    }
}

/// `F(x, y) ⋚ 0` over the view.
pub fn plot_implicit_inequality<F>(
    f: &F,
    cmp: Comparison,
    view: &View,
    opts: &PlotOptions,
) -> Result<Region, Cancelled>
where
    F: Fn(f64, f64) -> f64 + ?Sized,
{
    // Normalise to "inside means g < 0".
    let less = cmp.is_less();
    let g = |x: f64, y: f64| {
        let v = f(x, y);
        if less { v } else { -v }
    };
    let mut grid = Grid::build(&g, view, opts)?;
    let mut fill = merge_full_cells(&grid);
    for c in grid.mixed.clone() {
        cell_polygons(&mut grid, c, &mut fill)?;
    }
    let boundary = grid.contour()?;
    Ok(Region {
        fill: clip_fill(fill, view),
        boundary,
        boundary_style: cmp.boundary_style(),
        truncated: grid.truncated(),
    })
}

/// Rectangles for fully-inside cells, merging horizontal runs of equal size.
fn merge_full_cells<F>(grid: &Grid<'_, F>) -> Vec<Polygon>
where
    F: Fn(f64, f64) -> f64 + ?Sized,
{
    let mut cells = grid.full.clone();
    cells.sort_unstable_by_key(|c| (c.size, c.j, c.i));
    let mut out = Vec::new();
    let mut run: Option<(Cell, i64)> = None; // (first cell, end i)
    let emit = |out: &mut Vec<Polygon>, (c, end): (Cell, i64)| {
        let a = grid.pos((c.i, c.j));
        let b = grid.pos((end, c.j + c.size));
        out.push(Polygon::new(vec![
            a,
            Vec2::new(b.x, a.y),
            b,
            Vec2::new(a.x, b.y),
        ]));
    };
    for c in cells {
        match run {
            Some((first, end)) if first.size == c.size && first.j == c.j && end == c.i => {
                run = Some((first, end + c.size));
            }
            _ => {
                if let Some(r) = run.take() {
                    emit(&mut out, r);
                }
                run = Some((c, c.i + c.size));
            }
        }
    }
    if let Some(r) = run {
        emit(&mut out, r);
    }
    out
}

/// Inside part of a boundary cell.
fn cell_polygons<F>(
    grid: &mut Grid<'_, F>,
    c: Cell,
    out: &mut Vec<Polygon>,
) -> Result<(), Cancelled>
where
    F: Fn(f64, f64) -> f64 + ?Sized,
{
    let corners = c.corners();
    let edges = c.edges();
    let mut vals = [0.0; 4];
    let mut cross = [None; 4];
    for k in 0..4 {
        vals[k] = grid.value(corners[k])?;
        if vals[k].is_nan() {
            // Undefined corner: the domain edge inside the cell is unknown,
            // and joining the defined corners would cut across it.
            return Ok(());
        }
        cross[k] = grid.crossing(edges[k])?.point();
    }
    if cross.iter().all(Option::is_some) && !inside(grid.saddle_center(c)?) {
        // Saddle with separated inside corners: one triangle per corner.
        for k in 0..4 {
            if inside(vals[k])
                && let (Some(p), Some(q)) = (cross[(k + 3) % 4], cross[k])
            {
                out.push(Polygon::new(vec![p, grid.pos(corners[k]), q]));
            }
        }
        return Ok(());
    }
    let mut pts = Vec::with_capacity(8);
    for k in 0..4 {
        if inside(vals[k]) {
            pts.push(grid.pos(corners[k]));
        }
        if let Some(p) = cross[k] {
            pts.push(p);
        }
    }
    if pts.len() >= 3 {
        out.push(Polygon::new(pts));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;

    fn view() -> View {
        View::new(Rect::new(-3.0, -2.0, 3.0, 2.0), 1200.0, 800.0)
    }

    fn opts() -> PlotOptions {
        PlotOptions::default()
    }

    /// Fills are clipped to the view, so this is just the region area.
    fn visible_area(r: &Region, v: &View) -> f64 {
        let b = v.bounds();
        assert!(
            r.fill
                .iter()
                .flat_map(|p| p.points.iter())
                .all(|p| b.expanded(1e-12).contains(*p))
        );
        r.area()
    }

    fn contains(poly: &Polygon, p: Vec2) -> bool {
        let pts = &poly.points;
        let mut inside = false;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[j]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    #[test]
    fn comparison_flags() {
        assert!(Comparison::Less.is_strict() && Comparison::Greater.is_strict());
        assert!(!Comparison::LessEq.is_strict());
        assert_eq!(Comparison::Less.boundary_style(), LineStyle::Dashed);
        assert_eq!(Comparison::GreaterEq.boundary_style(), LineStyle::Solid);
        assert!(Comparison::LessEq.is_less() && !Comparison::GreaterEq.is_less());
    }

    #[test]
    fn below_sine() {
        let v = view();
        let r = plot_explicit_inequality(&f64::sin, Comparison::Less, &v, &opts()).unwrap();
        assert_eq!(r.fill.len(), 1);
        assert_eq!(r.boundary.len(), 1);
        assert_eq!(r.boundary_style, LineStyle::Dashed);
        // ∫_{-3}^{3} (sin x + 2) dx = 12 (sin is odd).
        let a = visible_area(&r, &v);
        assert!((a - 12.0).abs() < 0.1, "{a}");
    }

    #[test]
    fn above_parabola() {
        let v = view();
        let r =
            plot_explicit_inequality(&|x: f64| x * x, Comparison::GreaterEq, &v, &opts()).unwrap();
        assert_eq!(r.boundary_style, LineStyle::Solid);
        // Visible area above x² and below 2: ∫_{-√2}^{√2} (2 - x²) dx = 8√2/3.
        let a = visible_area(&r, &v);
        let expect = 8.0 * 2f64.sqrt() / 3.0;
        assert!((a - expect).abs() < 0.05, "{a} vs {expect}");
    }

    #[test]
    fn reciprocal_inequality_two_pieces() {
        let r = plot_explicit_inequality(&|x: f64| 1.0 / x, Comparison::LessEq, &view(), &opts())
            .unwrap();
        assert_eq!(r.fill.len(), 2);
        assert_eq!(r.boundary.len(), 2);
    }

    #[test]
    fn domain_gap_not_shaded() {
        let v = view();
        let r = plot_explicit_inequality(&f64::sqrt, Comparison::Less, &v, &opts()).unwrap();
        assert_eq!(r.fill.len(), 1);
        assert!(r.fill[0].points.iter().all(|p| p.x >= 0.0));
    }

    #[test]
    fn left_of_parabola_in_y() {
        let v = view();
        let r = plot_explicit_x_inequality(&|y: f64| y * y - 1.0, Comparison::Less, &v, &opts())
            .unwrap();
        assert_eq!(r.fill.len(), 1);
        // ∫_{-2}^{2} (y² - 1 + 3) dy = 16/3 + 8.
        let a = visible_area(&r, &v);
        let expect = 16.0 / 3.0 + 8.0;
        assert!((a - expect).abs() < 0.1, "{a} vs {expect}");
    }

    #[test]
    fn inside_unit_disk() {
        let v = view();
        let f = |x: f64, y: f64| x * x + y * y - 1.0;
        let r = plot_implicit_inequality(&f, Comparison::Less, &v, &opts()).unwrap();
        assert!(
            (r.area() - std::f64::consts::PI).abs() < 1e-3,
            "{}",
            r.area()
        );
        assert_eq!(r.boundary.len(), 1);
        assert!(r.boundary[0].closed);
        assert_eq!(r.boundary_style, LineStyle::Dashed);
    }

    #[test]
    fn outside_unit_disk() {
        let v = view();
        let f = |x: f64, y: f64| x * x + y * y - 1.0;
        let r = plot_implicit_inequality(&f, Comparison::GreaterEq, &v, &opts()).unwrap();
        // Cells cover the view rounded out to whole coarse cells.
        let a = visible_area(&r, &v);
        let expect = 24.0 - std::f64::consts::PI;
        assert!((a - expect).abs() < 0.05, "{a} vs {expect}");
    }

    #[test]
    fn lemniscate_area_with_saddle() {
        let f = |x: f64, y: f64| (x * x + y * y).powi(2) - 2.0 * (x * x - y * y);
        let r = plot_implicit_inequality(&f, Comparison::Less, &view(), &opts()).unwrap();
        assert!((r.area() - 2.0).abs() < 2e-3, "{}", r.area());
    }

    #[test]
    fn undefined_corners_are_not_shaded() {
        // y < sqrt(x): undefined for x < 0, must not shade across the y axis.
        let f = |x: f64, y: f64| y - x.sqrt();
        let v = view();
        let r = plot_implicit_inequality(&f, Comparison::Less, &v, &opts()).unwrap();
        assert!(
            r.fill
                .iter()
                .flat_map(|p| p.points.iter())
                .all(|p| p.x >= -1e-12)
        );
        // ∫_0^3 (sqrt(x) + 2) dx clipped at y = 2 is 6 + ∫_0^3 sqrt(x) dx = 6 + 2·3^1.5/3.
        let expect = 6.0 + 2.0 * 3f64.powf(1.5) / 3.0;
        assert!((r.area() - expect).abs() < 0.02, "{} vs {expect}", r.area());
    }

    #[test]
    fn truncated_region_flag() {
        let f = |x: f64, y: f64| x * x + y * y - 1.0;
        let o = PlotOptions {
            max_evals: 2_000,
            ..opts()
        };
        let r = plot_implicit_inequality(&f, Comparison::Less, &view(), &o).unwrap();
        assert!(r.truncated);
        assert!(
            !plot_implicit_inequality(&f, Comparison::Less, &view(), &opts())
                .unwrap()
                .truncated
        );
    }

    #[test]
    fn pole_boundary_filtered() {
        let f = |x: f64, y: f64| 1.0 / (x * y) - 1.0;
        let r = plot_implicit_inequality(&f, Comparison::Greater, &view(), &opts()).unwrap();
        assert_eq!(r.boundary.len(), 2);
        assert!(!r.fill.is_empty());
        // Region 1/(xy) > 1 ⇔ 0 < xy < 1: point (0.5, 0.5) inside, (2, 2) not.
        assert!(r.fill.iter().any(|p| contains(p, Vec2::new(0.5, 0.5))));
        assert!(!r.fill.iter().any(|p| contains(p, Vec2::new(2.0, 2.0))));
        assert!(!r.fill.iter().any(|p| contains(p, Vec2::new(-0.5, 0.5))));
    }

    #[test]
    fn region_primitives_order() {
        let f = |x: f64, y: f64| x * x + y * y - 1.0;
        let r = plot_implicit_inequality(&f, Comparison::LessEq, &view(), &opts()).unwrap();
        let prims = r.primitives();
        assert!(matches!(prims[0], Primitive::Polygon(_)));
        assert!(matches!(
            prims.last(),
            Some(Primitive::Polyline {
                style: LineStyle::Solid,
                ..
            })
        ));
    }
}
