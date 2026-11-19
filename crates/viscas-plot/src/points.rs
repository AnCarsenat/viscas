//! Points and point lists.

use crate::geom::{Polyline, Primitive, Vec2};
use crate::options::PlotOptions;
use crate::view::View;

/// Finite points that fall inside the view (plus the clip margin, so marker
/// discs at the edge are still drawn).
pub fn plot_points(points: &[Vec2], view: &View, opts: &PlotOptions) -> Vec<Vec2> {
    let rect = view.bounds().expanded(opts.clip_margin.max(0.0));
    points
        .iter()
        .copied()
        .filter(|&p| p.is_finite() && rect.contains(p))
        .collect()
}

/// A point list, optionally connected by straight segments (table plots).
/// Non-finite entries break the connecting line. Returns the connecting
/// polylines (clipped) followed by one `Points` primitive.
pub fn plot_point_list(
    points: &[Vec2],
    connect: bool,
    view: &View,
    opts: &PlotOptions,
) -> Vec<Primitive> {
    let mut out = Vec::new();
    if connect {
        let rect = view.bounds().expanded(opts.clip_margin.max(0.0));
        for run in points.split(|p| !p.is_finite()) {
            if run.len() >= 2 {
                for line in Polyline::open(run.to_vec()).clipped(&rect) {
                    out.push(Primitive::Polyline {
                        line,
                        style: Default::default(),
                    });
                }
            }
        }
    }
    let visible = plot_points(points, view, opts);
    if !visible.is_empty() {
        out.push(Primitive::Points(visible));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;

    fn view() -> View {
        View::new(Rect::new(0.0, 0.0, 10.0, 10.0), 500.0, 500.0)
    }

    #[test]
    fn filters_non_finite_and_far_points() {
        let pts = [
            Vec2::new(1.0, 1.0),
            Vec2::new(f64::NAN, 1.0),
            Vec2::new(100.0, 1.0),
            Vec2::new(11.0, 5.0), // inside the 25 % margin
        ];
        let v = plot_points(&pts, &view(), &PlotOptions::default());
        assert_eq!(v, vec![Vec2::new(1.0, 1.0), Vec2::new(11.0, 5.0)]);
    }

    #[test]
    fn connected_list_breaks_at_nan() {
        let pts = [
            Vec2::new(1.0, 1.0),
            Vec2::new(2.0, 3.0),
            Vec2::new(f64::NAN, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(5.0, 2.0),
            Vec2::new(6.0, 1.0),
        ];
        let prims = plot_point_list(&pts, true, &view(), &PlotOptions::default());
        let lines = prims
            .iter()
            .filter(|p| matches!(p, Primitive::Polyline { .. }))
            .count();
        assert_eq!(lines, 2);
        assert!(matches!(prims.last(), Some(Primitive::Points(v)) if v.len() == 5));
        let unconnected = plot_point_list(&pts, false, &view(), &PlotOptions::default());
        assert_eq!(unconnected.len(), 1);
    }
}
