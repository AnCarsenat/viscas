//! Plot pipeline: primitives per kind, jobs, cancellation, caching, colors.

use std::time::Duration;

use viscas_plot::{PlotOptions, Primitive, Rect, View};
use viscas_scene::{Color, NodeContent, PropValue, Style};

use super::*;

fn moved() -> View {
    View::new(Rect::new(0.0, 0.0, 5.0, 5.0), 400.0, 400.0)
}

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
    // Property edits dirty plot nodes (regression: they used not to).
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
    let vp = e.create_viewport();
    assert_eq!(e.request_plots(vp, view()), ids.len());
    assert!(e.wait_for_plots(Duration::from_secs(30)));
    let layers = e.layers(vp);
    assert_eq!(layers.len(), ids.len());
    for l in &layers {
        assert!(!l.primitives.is_empty(), "{} empty", l.node_id);
    }
    assert!(
        layers
            .windows(2)
            .all(|w| w[0].kind.draw_rank() <= w[1].kind.draw_rank())
    );
    assert_eq!(e.pending_plots(vp), 0);
}

#[test]
fn poll_reports_updated_viewports() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = e.create_viewport();
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
    let vp = e.create_viewport();
    let old = e.prepare_job(vp, view());
    let new = e.prepare_job(vp, moved());
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
    let vp = e.create_viewport();
    let old = e.prepare_job(vp, view());
    let layer = old.run().unwrap().remove(0);
    let _new = e.prepare_job(vp, moved());
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
    let (v1, v2) = (e.create_viewport(), e.create_viewport());
    let a = e.prepare_job(v1, view());
    let _b = e.prepare_job(v2, moved());
    assert!(!a.cancel_token().is_cancelled());
}

#[test]
fn cache_hit_skips_rendering() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = e.create_viewport();
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
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    set(&mut e, a, "2");
    // The old layer stays drawable, marked stale (regression: it vanished).
    let old = e.layers(vp).into_iter().find(|l| l.node_id == p).unwrap();
    assert!(old.stale);
    assert_eq!(e.needs_replot(), vec![vp]);
    let job = e.prepare_job(vp, view());
    let nodes: Vec<NodeId> = job.items.iter().map(|i| i.node).collect();
    assert_eq!(nodes, vec![p], "only the dependent plot is re-rendered");
    let _ = q;
}

#[test]
fn view_change_marks_layers_stale_until_rerendered() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = e.create_viewport();
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
    let vp = e.create_viewport();
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
    let vp = e.create_viewport();
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
    assert_eq!(e.node_color(a), Some(DEFAULT_PALETTE[0]));
    assert_eq!(e.node_color(b), Some(DEFAULT_PALETTE[1]));
    // Automatic colors are not written into the (saved) style.
    assert_eq!(e.scene().get(a).unwrap().style.color, None);
}

#[test]
fn injected_palette_is_used_and_cycles() {
    let mut e = engine();
    let theme = vec![Color::rgb(1, 2, 3), Color::rgb(4, 5, 6)];
    e.set_palette(theme.clone());
    let ids: Vec<NodeId> = (0..3).map(|i| plot(&mut e, &format!("y = {i}"))).collect();
    let colors: Vec<Color> = ids.iter().map(|id| e.node_color(*id).unwrap()).collect();
    assert_eq!(colors, vec![theme[0], theme[1], theme[0]]);
    // A new theme recolors automatic colors.
    let theme2 = vec![Color::rgb(7, 7, 7), Color::rgb(8, 8, 8)];
    e.set_palette(theme2.clone());
    assert_eq!(e.node_color(ids[0]), Some(theme2[0]));
    assert_eq!(e.node_color(ids[1]), Some(theme2[1]));
    e.set_palette(theme.clone());
    e.set_palette(Vec::new());
    assert_eq!(e.palette(), &theme[..]);
}

#[test]
fn adding_a_plot_is_one_undo_step() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    assert!(e.undo());
    assert!(!e.scene().contains(p));
    assert!(!e.scene().can_undo());
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
    let vp = e.create_viewport();
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
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    let layers = std::thread::spawn(move || job.run().unwrap())
        .join()
        .unwrap();
    assert_eq!(layers.len(), 1);
    assert!(!layers[0].primitives.is_empty());
}

#[test]
fn viewports_are_allocated_by_the_engine() {
    let mut e = engine();
    let a = e.create_viewport();
    let b = e.create_viewport();
    assert_ne!(a, b);
    let vnode = e.add_node(None, NodeContent::viewport("View")).unwrap();
    let v = e.viewport_for_node(vnode);
    assert_eq!(e.viewport_for_node(vnode), v);
    assert_eq!(e.node_viewport(vnode), Some(v));
    assert!(v != a && v != b);
    e.close_viewport(v);
    assert_eq!(e.node_viewport(vnode), None);
}

#[test]
fn unknown_viewport_gets_an_empty_job() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let mut other = engine();
    let foreign = other.create_viewport();
    let _ = other.create_viewport();
    let job = e.prepare_job(ViewportId(foreign.raw() + 100), view());
    assert!(job.items.is_empty());
}

#[test]
fn plot_kind_names() {
    assert_eq!(PlotKind::ExplicitY.to_string(), "explicit y");
    assert_eq!(PlotKind::PolarInequality.as_str(), "polar inequality");
}

// ----------------------------------------------------------------------
// Review regressions
// ----------------------------------------------------------------------

/// A plot whose samples are slow and use the general evaluator.
fn slow_plot(e: &mut Engine) -> NodeId {
    plot(e, "y = ∑(k = 1, 20000) sin(k x) / k^2")
}

#[test]
fn slow_plots_use_interruptible_closures() {
    let mut e = engine();
    let p = slow_plot(&mut e);
    assert!(e.plot_of(p).unwrap().rebuild.is_some());
    let fast = plot(&mut e, "y = x");
    assert!(e.plot_of(fast).unwrap().rebuild.is_none());
}

#[test]
fn cancelling_a_job_interrupts_a_slow_sample() {
    let mut e = engine();
    slow_plot(&mut e);
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    let cancel = job.cancel.clone();
    let start = std::time::Instant::now();
    let h = std::thread::spawn(move || job.run());
    std::thread::sleep(Duration::from_millis(50));
    cancel.cancel();
    let r = h.join().unwrap();
    assert!(r.is_err(), "a cancelled job never yields layers");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    assert!(e.layers(vp).is_empty());
}

#[test]
fn dropping_the_engine_does_not_hang() {
    let mut e = engine();
    for _ in 0..3 {
        slow_plot(&mut e);
    }
    let vp = e.create_viewport();
    e.request_plots(vp, view());
    std::thread::sleep(Duration::from_millis(50));
    let start = std::time::Instant::now();
    drop(e);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn queue_does_not_grow_with_superseded_jobs() {
    let mut e = Engine::with_workers(1);
    for _ in 0..4 {
        slow_plot(&mut e);
    }
    let vp = e.create_viewport();
    for i in 0..50 {
        let v = View::new(Rect::new(-10.0 - i as f64, -10.0, 10.0, 10.0), 400.0, 400.0);
        e.request_plots(vp, v);
    }
    assert!(e.queued_tasks() <= 4, "{}", e.queued_tasks());
}

#[test]
fn same_view_request_is_a_noop_while_running() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = e.create_viewport();
    let first = e.prepare_job(vp, view());
    assert_eq!(first.items.len(), 1);
    let again = e.prepare_job(vp, view());
    assert!(again.items.is_empty());
    assert!(!first.cancel_token().is_cancelled(), "not cancelled");
    assert!(e.run_job_now(&first));
    assert_eq!(e.layers(vp).len(), 1);
}

#[test]
fn same_view_request_adds_only_new_nodes() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = e.create_viewport();
    let first = e.prepare_job(vp, view());
    let q = plot(&mut e, "y = 2");
    let second = e.prepare_job(vp, view());
    let nodes: Vec<NodeId> = second.items.iter().map(|i| i.node).collect();
    assert_eq!(nodes, vec![q]);
    assert!(e.run_job_now(&first) && e.run_job_now(&second));
    assert_eq!(e.layers(vp).len(), 2);
}

#[test]
fn generations_are_engine_wide() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let (a, b) = (e.create_viewport(), e.create_viewport());
    let ja = e.prepare_job(a, view());
    let jb = e.prepare_job(b, view());
    let ja2 = e.prepare_job(a, moved());
    assert!(ja.generation < jb.generation && jb.generation < ja2.generation);
}

#[test]
fn set_plot_options_cancels_in_flight_jobs() {
    let mut e = engine();
    plot(&mut e, "y = x");
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    e.set_plot_options(PlotOptions::default());
    assert!(job.cancel_token().is_cancelled());
    assert!(!e.run_job_now(&job));
    assert_eq!(e.prepare_job(vp, view()).items.len(), 1);
}

#[test]
fn error_layers_are_retried() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    let mut layer = job.run().unwrap().remove(0);
    layer.error = Some("boom".into());
    layer.primitives = std::sync::Arc::new(Vec::new());
    assert!(e.accept(PlotResult {
        viewport: vp,
        generation: job.generation,
        revision: e.evaluator().revision(p),
        view: view(),
        layer,
    }));
    assert_eq!(e.layers(vp)[0].error.as_deref(), Some("boom"));
    // Not a replot trigger for the same view and revision (no loop)...
    assert!(e.needs_replot().is_empty());
    // ...but an explicit request retries it.
    assert_eq!(e.prepare_job(vp, view()).items.len(), 1, "retried");
}

#[test]
fn needs_replot_without_views_is_empty_and_pending_work_tracks_jobs() {
    let mut e = engine();
    plot(&mut e, "y = x");
    assert!(e.needs_replot().is_empty());
    assert!(!e.has_pending_work());
    let vp = e.create_viewport();
    assert!(e.request_plots(vp, view()) > 0);
    assert!(e.has_pending_work());
    assert!(e.wait_for_plots(std::time::Duration::from_secs(10)));
    assert!(!e.has_pending_work());
    assert!(e.needs_replot().is_empty());
}

#[test]
fn approx_toggle_does_not_reevaluate() {
    let mut e = engine();
    let p = plot(&mut e, "y = x");
    let rev = e.evaluator().revision(p);
    e.drain_events();
    let s = EngineSettings {
        approx: true,
        ..*e.settings()
    };
    e.set_settings(s);
    assert!(e.settings().approx);
    assert_eq!(e.evaluator().revision(p), rev);
    let s = EngineSettings {
        angle: viscas_core::eval::AngleUnit::Degrees,
        ..*e.settings()
    };
    e.set_settings(s);
    assert_ne!(e.evaluator().revision(p), rev, "angle changes re-evaluate");
}

#[test]
fn layers_share_geometry() {
    let mut e = engine();
    plot(&mut e, "y = x^2");
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    let a = e.layers(vp);
    let b = e.layers(vp);
    assert!(std::sync::Arc::ptr_eq(&a[0].primitives, &b[0].primitives));
}

#[test]
fn layers_draw_fills_then_curves_then_points() {
    let mut e = engine();
    let pt = plot(&mut e, "(1, 1)");
    let curve = plot(&mut e, "y = x");
    let fill = plot(&mut e, "y < x");
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    let order: Vec<NodeId> = e.layers(vp).iter().map(|l| l.node_id).collect();
    assert_eq!(order, vec![fill, curve, pt]);
    assert_eq!(e.layers(vp)[0].kind, PlotKind::Inequality);
}

#[test]
fn needs_replot_tracks_views_and_changes() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    plot(&mut e, "y = a x");
    let vp = e.create_viewport();
    assert!(e.needs_replot().is_empty(), "no view yet");
    let job = e.prepare_job(vp, view());
    assert!(e.needs_replot().is_empty(), "in flight");
    e.run_job_now(&job);
    assert!(e.needs_replot().is_empty());
    set(&mut e, a, "3");
    assert_eq!(e.needs_replot(), vec![vp]);
}

#[test]
fn async_features_are_delivered() {
    let mut e = engine();
    plot(&mut e, "y = x^2 - 1");
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    let t1 = e
        .request_features(vp, viscas_plot::Vec2::new(1.0, 0.0), 0.3)
        .unwrap();
    let t2 = e
        .request_features(vp, viscas_plot::Vec2::new(1.0, 0.0), 0.3)
        .unwrap();
    assert!(t2 > t1);
    let r = e
        .wait_for_features(Duration::from_secs(10))
        .expect("result");
    assert_eq!(r.ticket, t2, "older requests are superseded");
    assert!(
        r.hits
            .iter()
            .any(|h| h.feature.kind == viscas_plot::FeatureKind::Root)
    );
    let other = e.create_viewport();
    assert!(
        e.request_features(other, viscas_plot::Vec2::new(0.0, 0.0), 1.0)
            .is_none()
    );
}
