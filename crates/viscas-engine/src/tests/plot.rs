//! Plot pipeline: primitives per kind, jobs, cancellation, caching, colors.

use std::time::Duration;

use viscas_plot::{PlotOptions, Primitive, Rect, View};
use viscas_scene::{Color, NodeContent, PropValue, Style};

use super::*;

fn all_points(l: &PlotLayer) -> Vec<viscas_plot::Vec2> {
    l.primitives
        .iter()
        .flat_map(|p| match p {
            Primitive::Polyline { line, .. } => line.points.clone(),
            Primitive::Polygon(poly) => poly.points.clone(),
            Primitive::Points(pts) => pts.clone(),
        })
        .collect()
}

#[test]
fn explicit_y_produces_polylines() {
    let mut e = engine();
    let p = plot(&mut e, "y = sin x");
    let l = layer(&mut e, p);
    assert!(polylines(&l) >= 1);
    assert!(l.error.is_none());
    assert!(!l.stale);
}

#[test]
fn explicit_x_produces_polylines() {
    let mut e = engine();
    let p = plot(&mut e, "x = y^2 - 3");
    let l = layer(&mut e, p);
    assert!(polylines(&l) >= 1);
    assert!(all_points(&l).iter().any(|q| q.x < -2.0));
}

#[test]
fn parametric_default_range_is_full_circle() {
    let mut e = engine();
    let p = plot(&mut e, "(2 cos t, 2 sin t)");
    let l = layer(&mut e, p);
    let pts = all_points(&l);
    assert!(!pts.is_empty());
    assert!(pts.iter().all(|q| (q.length() - 2.0).abs() < 1e-6));
    assert!(pts.iter().any(|q| q.y < -1.9));
}

#[test]
fn parametric_range_from_props() {
    let mut e = engine();
    let p = plot(&mut e, "(cos t, sin t)");
    e.scene_mut().set_prop(p, "t_min", 0.0).unwrap();
    e.scene_mut().set_prop(p, "t_max", "pi").unwrap();
    // Props do not dirty the node by themselves.
    e.scene_mut().mark_dirty(p);
    e.update();
    let l = layer(&mut e, p);
    assert!(all_points(&l).iter().all(|q| q.y >= -1e-9));
}

#[test]
fn bad_range_prop_is_an_error() {
    let mut e = engine();
    let p = plot(&mut e, "(cos t, sin t)");
    e.scene_mut()
        .set_prop(p, "t_max", PropValue::Bool(true))
        .unwrap();
    e.scene_mut().mark_dirty(p);
    e.update();
    assert!(e.scene().status(p).is_error());
}

#[test]
fn polar_curve() {
    let mut e = engine();
    let p = plot(&mut e, "r = 3");
    let l = layer(&mut e, p);
    let pts = all_points(&l);
    assert!(!pts.is_empty());
    assert!(pts.iter().all(|q| (q.length() - 3.0).abs() < 1e-6));
}

#[test]
fn polar_curve_in_degree_mode() {
    let mut e = engine();
    e.set_settings(EngineSettings {
        angle: viscas_core::eval::AngleUnit::Degrees,
        ..*e.settings()
    });
    // r = θ/90 reaches r = 4 at θ = 360°.
    let p = plot(&mut e, "r = θ / 90");
    let l = layer(&mut e, p);
    let max = all_points(&l)
        .iter()
        .map(|q| q.length())
        .fold(0.0, f64::max);
    assert!((max - 4.0).abs() < 0.05, "{max}");
}

#[test]
fn polar_angle_ray() {
    let mut e = engine();
    let p = plot(&mut e, "θ = pi/4");
    let l = layer(&mut e, p);
    let pts = all_points(&l);
    assert!(!pts.is_empty());
    assert!(pts.iter().all(|q| (q.x - q.y).abs() < 1e-6));
}

#[test]
fn implicit_circle() {
    let mut e = engine();
    let p = plot(&mut e, "x^2 + y^2 = 16");
    let l = layer(&mut e, p);
    let pts = all_points(&l);
    assert!(!pts.is_empty());
    assert!(pts.iter().all(|q| (q.length() - 4.0).abs() < 0.1));
}

#[test]
fn explicit_inequality_fills() {
    let mut e = engine();
    let p = plot(&mut e, "y < x");
    let l = layer(&mut e, p);
    assert!(polygons(&l) >= 1);
    assert!(matches!(
        e.plot_of(p).unwrap().shape,
        PlotShape::InequalityY { .. }
    ));
}

#[test]
fn flipped_explicit_inequality() {
    let mut e = engine();
    let p = plot(&mut e, "x^2 > y");
    assert!(matches!(
        e.plot_of(p).unwrap().shape,
        PlotShape::InequalityY {
            cmp: viscas_plot::Comparison::Less,
            ..
        }
    ));
    assert!(polygons(&layer(&mut e, p)) >= 1);
}

#[test]
fn x_inequality_fills() {
    let mut e = engine();
    let p = plot(&mut e, "x >= y^2");
    assert!(matches!(
        e.plot_of(p).unwrap().shape,
        PlotShape::InequalityX { .. }
    ));
    assert!(polygons(&layer(&mut e, p)) >= 1);
}

#[test]
fn implicit_inequality_fills() {
    let mut e = engine();
    let p = plot(&mut e, "x^2 + y^2 < 4");
    let l = layer(&mut e, p);
    assert!(polygons(&l) >= 1);
    for q in all_points(&l) {
        assert!(q.length() < 2.3, "{q:?}");
    }
}

#[test]
fn polar_inequality_fills_disc() {
    let mut e = engine();
    let p = plot(&mut e, "r < 2");
    let l = layer(&mut e, p);
    assert!(polygons(&l) >= 1);
    for q in all_points(&l) {
        assert!(q.length() < 2.3, "{q:?}");
    }
}

#[test]
fn chained_polar_inequality_is_a_sector() {
    let mut e = engine();
    let p = plot(&mut e, "0 < θ < pi/2");
    let l = layer(&mut e, p);
    assert!(polygons(&l) >= 1);
    for q in all_points(&l) {
        assert!(q.x >= -0.2 && q.y >= -0.2, "{q:?}");
    }
}

#[test]
fn chained_cartesian_inequality_is_a_band() {
    let mut e = engine();
    let p = plot(&mut e, "-1 < x < 1");
    let l = layer(&mut e, p);
    assert!(polygons(&l) >= 1);
    for q in all_points(&l) {
        assert!(q.x.abs() < 1.3, "{q:?}");
    }
}

#[test]
fn restriction_limits_the_curve() {
    let mut e = engine();
    let p = plot(&mut e, "y = x {x > 0}");
    let l = layer(&mut e, p);
    let pts = all_points(&l);
    assert!(!pts.is_empty());
    assert!(pts.iter().all(|q| q.x >= -0.1));
}

#[test]
fn point_plot_and_point_list() {
    let mut e = engine();
    let p = plot(&mut e, "(1, 2)");
    let l = layer(&mut e, p);
    assert!(matches!(&l.primitives[..], [Primitive::Points(v)] if v.len() == 1));
    let q = plot(&mut e, "[(1, 2), (3, 4), (5, 6)]");
    let l = layer(&mut e, q);
    assert_eq!(all_points(&l).len(), 3);
}

#[test]
fn point_node_is_drawn() {
    let mut e = engine();
    let p = e.add_node(None, NodeContent::point("P", "(2, 3)")).unwrap();
    let l = layer(&mut e, p);
    assert_eq!(all_points(&l), vec![viscas_plot::Vec2::new(2.0, 3.0)]);
}

#[test]
fn every_kind_renders_on_the_pool() {
    let mut e = engine();
    let rels = [
        "y = x^2",
        "x = sin y",
        "(cos t, sin t)",
        "r = θ / 4",
        "θ = 1",
        "x y = 1",
        "y > x^3",
        "x^2 + y^2 <= 9",
        "r <= 1",
        "(3, 3)",
    ];
    let ids: Vec<NodeId> = rels.iter().map(|r| plot(&mut e, r)).collect();
    let vp = ViewportId(7);
    assert_eq!(e.request_plots(vp, view()), ids.len());
    assert!(e.wait_for_plots(Duration::from_secs(30)));
    let layers = e.layers(vp);
    assert_eq!(layers.len(), ids.len());
    for (l, id) in layers.iter().zip(&ids) {
        assert_eq!(l.node_id, *id, "draw order follows the scene");
        assert!(!l.primitives.is_empty(), "{id} empty");
    }
    assert_eq!(e.pending_plots(vp), 0);
}

#[test]
fn poll_reports_updated_viewports() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = ViewportId(3);
    e.request_plots(vp, view());
    let mut got = Vec::new();
    for _ in 0..3000 {
        got.extend(e.poll_plots());
        if !got.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(got, vec![vp]);
}

#[test]
fn newer_job_cancels_older() {
    let mut e = engine();
    plot(&mut e, "y = x^2");
    let vp = ViewportId(1);
    let old = e.prepare_job(vp, view());
    let new = e.prepare_job(vp, view());
    assert!(old.cancel_token().is_cancelled());
    assert!(!new.cancel_token().is_cancelled());
    assert!(new.generation > old.generation);
    assert!(old.run().is_err());
    assert!(!e.run_job_now(&old));
    assert!(e.layers(vp).is_empty());
    assert!(e.run_job_now(&new));
    assert_eq!(e.layers(vp).len(), 1);
}

#[test]
fn stale_generation_results_are_discarded() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    let vp = ViewportId(1);
    let old = e.prepare_job(vp, view());
    let layer = PlotJob {
        cancel: viscas_plot::CancelToken::new(),
        ..old.clone()
    }
    .run()
    .unwrap()
    .remove(0);
    let _new = e.prepare_job(vp, view());
    let accepted = e.accept(PlotResult {
        viewport: vp,
        generation: old.generation,
        revision: e.evaluator().revision(p),
        view: view(),
        layer,
    });
    assert!(!accepted);
    assert!(e.layers(vp).is_empty());
}

#[test]
fn other_viewports_are_not_cancelled() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let a = e.prepare_job(ViewportId(1), view());
    let _b = e.prepare_job(ViewportId(2), view());
    assert!(!a.cancel_token().is_cancelled());
}

#[test]
fn cache_hit_skips_rendering() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    assert_eq!(job.items.len(), 1);
    e.run_job_now(&job);
    let again = e.prepare_job(vp, view());
    assert!(again.items.is_empty());
    assert_eq!(e.layers(vp).len(), 1, "cached layer survives the new job");
    assert_eq!(e.request_plots(vp, view()), 0);
}

#[test]
fn dirty_node_invalidates_cache() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let p = plot(&mut e, "y = a x");
    let q = plot(&mut e, "y = 5");
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    set(&mut e, a, "2");
    assert!(e.layers(vp).iter().all(|l| l.node_id != p));
    let job = e.prepare_job(vp, view());
    let nodes: Vec<NodeId> = job.items.iter().map(|i| i.node).collect();
    assert_eq!(nodes, vec![p], "only the dependent plot is re-rendered");
    let _ = q;
}

#[test]
fn view_change_marks_layers_stale_until_rerendered() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    let moved = View::new(Rect::new(0.0, 0.0, 5.0, 5.0), 400.0, 400.0);
    let job = e.prepare_job(vp, moved);
    assert_eq!(job.items.len(), 1);
    let layers = e.layers(vp);
    assert_eq!(layers.len(), 1);
    assert!(layers[0].stale);
    e.run_job_now(&job);
    assert!(!e.layers(vp)[0].stale);
}

#[test]
fn hidden_nodes_are_not_plotted() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    e.scene_mut().set_visible(p, false).unwrap();
    e.update();
    assert!(e.plot_items().is_empty());
    let folder = e.add_node(None, NodeContent::folder("F")).unwrap();
    let q = e
        .add_node(Some(folder), NodeContent::plot("y = 2"))
        .unwrap();
    assert_eq!(e.plot_items().len(), 1);
    e.scene_mut().set_visible(folder, false).unwrap();
    e.update();
    assert!(e.plot_items().iter().all(|i| i.node != q));
}

#[test]
fn removed_node_loses_its_layer() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    e.scene_mut().remove(p).unwrap();
    e.update();
    assert!(e.layers(vp).is_empty());
}

#[test]
fn style_change_applies_without_rerender() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    let red = Color::rgb(255, 0, 0);
    e.scene_mut()
        .set_style(
            p,
            Style {
                color: Some(red),
                line_width: 4.0,
                ..Style::default()
            },
        )
        .unwrap();
    e.update();
    let l = &e.layers(vp)[0];
    assert_eq!(l.style.color, red);
    assert_eq!(l.style.line_width, 4.0);
    assert!(e.prepare_job(vp, view()).items.is_empty());
}

#[test]
fn new_plots_get_palette_colors() {
    let mut e = engine();
    let a = plot(&mut e, "y = x");
    let b = plot(&mut e, "y = 2x");
    let ca = e.scene().get(a).unwrap().style.color;
    let cb = e.scene().get(b).unwrap().style.color;
    assert_eq!(ca, Some(DEFAULT_PALETTE[0]));
    assert_eq!(cb, Some(DEFAULT_PALETTE[1]));
}

#[test]
fn injected_palette_is_used_and_cycles() {
    let mut e = engine();
    let theme = vec![Color::rgb(1, 2, 3), Color::rgb(4, 5, 6)];
    e.set_palette(theme.clone());
    let ids: Vec<NodeId> = (0..3).map(|i| plot(&mut e, &format!("y = {i}"))).collect();
    let colors: Vec<Color> = ids
        .iter()
        .map(|id| e.scene().get(*id).unwrap().style.color.unwrap())
        .collect();
    assert_eq!(colors, vec![theme[0], theme[1], theme[0]]);
    e.set_palette(Vec::new());
    assert_eq!(e.palette(), &theme[..]);
}

#[test]
fn color_assignment_is_part_of_the_add_undo_step() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    assert!(e.undo());
    assert!(!e.scene().contains(p));
}

#[test]
fn nodes_without_color_get_an_automatic_one() {
    let mut e = engine();
    let p = e.scene_mut().add(None, NodeContent::plot("y = x")).unwrap();
    e.update();
    assert!(e.scene().get(p).unwrap().style.color.is_none());
    let c = e.node_color(p).unwrap();
    assert_eq!(layer(&mut e, p).style.color, c);
}

#[test]
fn explicit_style_color_is_kept() {
    let mut e = engine();
    let mut c = NodeContent::plot("y = x");
    c.style.color = Some(Color::rgb(9, 9, 9));
    let p = e.add_node(None, c).unwrap();
    assert_eq!(e.node_color(p), Some(Color::rgb(9, 9, 9)));
}

#[test]
fn truncation_is_reported() {
    let mut e = engine();
    let p = plot(&mut e, "y = sin(1/x)");
    e.set_plot_options(PlotOptions {
        max_evals: 50,
        ..PlotOptions::default()
    });
    assert!(layer(&mut e, p).truncated);
}

#[test]
fn close_viewport_drops_cache() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    e.close_viewport(vp);
    assert!(e.layers(vp).is_empty());
    assert!(e.viewport_view(vp).is_none());
}

#[test]
fn errored_plot_has_no_layer() {
    let mut e = engine();
    let p = plot(&mut e, "y = qq x");
    assert!(e.plot_items().iter().all(|i| i.node != p));
}

#[test]
fn plot_jobs_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PlotJob>();
    assert_send_sync::<PlotResult>();
    assert_send_sync::<CompiledPlot>();
}

#[test]
fn job_runs_on_another_thread() {
    let mut e = engine();
    plot(&mut e, "y = x^2");
    let job = e.prepare_job(ViewportId(1), view());
    let layers = std::thread::spawn(move || job.run().unwrap())
        .join()
        .unwrap();
    assert_eq!(layers.len(), 1);
    assert!(!layers[0].primitives.is_empty());
}

#[test]
fn viewport_id_from_node() {
    assert_eq!(ViewportId::from(NodeId(5)), ViewportId(5));
}

#[test]
fn plot_kind_names() {
    assert_eq!(PlotKind::ExplicitY.to_string(), "explicit y");
    assert_eq!(PlotKind::PolarInequality.as_str(), "polar inequality");
}
