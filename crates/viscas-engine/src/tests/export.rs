//! Headless export rendering: ranges, discontinuities, clipping.

use std::f64::consts::PI;
use std::sync::Arc;

use viscas_plot::{Comparison, Primitive, Rect, Vec2};
use viscas_scene::{Color, NodeId};

use crate::export::{
    ExportItem, ExportJob, ExportRange, clip_primitives, nearest_param, point_at, sample_table,
    trim_x,
};
use crate::{CompiledPlot, PlotItem, PlotKind, PlotShape, PlotStyle};

fn item(kind: PlotKind, shape: PlotShape) -> PlotItem {
    PlotItem {
        node: NodeId(7),
        revision: 0,
        style: PlotStyle::from_style(&Default::default(), Color::rgb(10, 20, 30)),
        plot: Arc::new(CompiledPlot::new(kind, shape)),
    }
}

fn polylines(prims: &[Primitive]) -> Vec<Vec<Vec2>> {
    prims
        .iter()
        .filter_map(|p| match p {
            Primitive::Polyline { line, .. } => Some(line.points.clone()),
            _ => None,
        })
        .collect()
}

fn run(it: ExportItem, world: Rect) -> Vec<Primitive> {
    let job = ExportJob::new(vec![it], world, (400.0, 400.0), 1.0);
    let layers = job.run(|_, _| {}).unwrap();
    layers[0].primitives.to_vec()
}

#[test]
fn identity_on_unit_interval_is_a_straight_line() {
    let it = ExportItem::new(item(
        PlotKind::ExplicitY,
        PlotShape::ExplicitY(Arc::new(|x| x)),
    ))
    .with_range(ExportRange::Param(0.0, 1.0));
    let lines = polylines(&run(it, Rect::new(-2.0, -2.0, 2.0, 2.0)));
    assert_eq!(lines.len(), 1);
    let l = &lines[0];
    assert!(l[0].x.abs() < 1e-12 && l[0].y.abs() < 1e-12);
    let last = l[l.len() - 1];
    assert!((last.x - 1.0).abs() < 1e-12 && (last.y - 1.0).abs() < 1e-12);
    assert!(
        l.iter()
            .all(|p| (p.x - p.y).abs() < 1e-12 && (0.0..=1.0).contains(&p.x))
    );
}

#[test]
fn part_of_a_curve_keeps_discontinuities_apart() {
    let it = ExportItem::new(item(
        PlotKind::ExplicitY,
        PlotShape::ExplicitY(Arc::new(|x| 1.0 / x)),
    ))
    .with_range(ExportRange::X(-1.0, 1.0));
    let lines = polylines(&run(it, Rect::new(-3.0, -10.0, 3.0, 10.0)));
    assert_eq!(lines.len(), 2, "one piece per branch");
    for l in &lines {
        let neg = l[0].x < 0.0;
        assert!(
            l.iter()
                .all(|p| (p.x < 0.0) == neg && p.x.abs() <= 1.0 + 1e-9)
        );
    }
}

#[test]
fn parametric_range_is_trimmed_to_the_parameter() {
    let shape = PlotShape::Parametric {
        x: Arc::new(f64::cos),
        y: Arc::new(f64::sin),
        range: (0.0, 2.0 * PI),
    };
    let it = ExportItem::new(item(PlotKind::Parametric, shape))
        .with_range(ExportRange::Param(0.0, PI / 2.0));
    let pts: Vec<Vec2> = polylines(&run(it, Rect::new(-2.0, -2.0, 2.0, 2.0))).concat();
    assert!(!pts.is_empty());
    // Quarter circle in the first quadrant only.
    assert!(pts.iter().all(|p| p.x >= -1e-9 && p.y >= -1e-9));
    assert!(pts.iter().all(|p| (p.length() - 1.0).abs() < 1e-6));
}

#[test]
fn region_is_clipped_to_the_box() {
    let shape = PlotShape::InequalityY {
        f: Arc::new(|x| x),
        cmp: Comparison::Less,
    };
    let clip = Rect::new(0.0, -1.0, 1.0, 1.0);
    let it = ExportItem::new(item(PlotKind::Inequality, shape)).with_clip(Some(clip));
    let prims = run(it, Rect::new(-5.0, -5.0, 5.0, 5.0));
    let mut area = 0.0;
    for p in &prims {
        match p {
            Primitive::Polygon(poly) => {
                assert!(poly.points.iter().all(|q| clip.contains(*q)));
                area += poly.area();
            }
            Primitive::Polyline { line, .. } => {
                assert!(line.points.iter().all(|q| clip.contains(*q)));
            }
            Primitive::Points(_) => {}
        }
    }
    // y < x inside [0,1]×[-1,1]: 1 (below y = 0) + 1/2.
    assert!((area - 1.5).abs() < 0.02, "area {area}");
}

#[test]
fn trimming_cuts_at_the_bounds() {
    let line = Primitive::Polyline {
        line: viscas_plot::Polyline::open(vec![Vec2::new(-2.0, 0.0), Vec2::new(2.0, 4.0)]),
        style: Default::default(),
    };
    let lines = polylines(&trim_x(&[line], 0.0, 1.0));
    assert_eq!(lines, vec![vec![Vec2::new(0.0, 2.0), Vec2::new(1.0, 3.0)]]);
    let pts = Primitive::Points(vec![Vec2::new(0.5, 0.5), Vec2::new(5.0, 0.0)]);
    let kept = clip_primitives(&[pts], &Rect::new(0.0, 0.0, 1.0, 1.0));
    assert_eq!(kept, vec![Primitive::Points(vec![Vec2::new(0.5, 0.5)])]);
}

#[test]
fn table_samples_and_nearest_parameter() {
    let shape = PlotShape::ExplicitY(Arc::new(|x: f64| x.sqrt()));
    let pieces = sample_table(&shape, (-1.0, 1.0), 5).unwrap();
    // sqrt is undefined below 0: only t = 0, 0.5, 1 remain, in one piece.
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].len(), 3);
    assert_eq!(pieces[0][0], [0.0, 0.0, 0.0]);
    assert_eq!(point_at(&shape, 4.0), Some(Vec2::new(4.0, 2.0)));
    let t = nearest_param(&shape, (0.0, 4.0), Vec2::new(4.1, 2.0), (1.0, 1.0), 400).unwrap();
    assert!((t - 4.0).abs() < 0.05);
    let implicit = PlotShape::Implicit(Arc::new(|x, y| x * x + y * y - 1.0));
    assert!(sample_table(&implicit, (0.0, 1.0), 10).is_none());
}

#[test]
fn cancelled_jobs_stop() {
    let it = ExportItem::new(item(
        PlotKind::ExplicitY,
        PlotShape::ExplicitY(Arc::new(|x| x)),
    ));
    let job = ExportJob::new(
        vec![it],
        Rect::new(-1.0, -1.0, 1.0, 1.0),
        (100.0, 100.0),
        1.0,
    );
    job.cancel_token().cancel();
    assert!(job.run(|_, _| {}).is_err());
}
