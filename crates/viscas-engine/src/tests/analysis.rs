//! Click-to-show features.

use viscas_plot::{FeatureKind, Vec2};

use super::*;

fn plotted(e: &mut Engine, rels: &[&str]) -> (Vec<NodeId>, ViewportId) {
    let ids = rels.iter().map(|r| plot(e, r)).collect();
    let vp = e.create_viewport();
    let job = e.prepare_job(vp, view());
    e.run_job_now(&job);
    (ids, vp)
}

#[test]
fn finds_root_of_parabola_near_one() {
    let mut e = engine();
    let (ids, vp) = plotted(&mut e, &["y = x^2 - 1"]);
    let hits = e.features_near(vp, Vec2::new(1.05, 0.02), 0.5);
    let root = hits
        .iter()
        .find(|h| h.feature.kind == FeatureKind::Root)
        .expect("root");
    assert_eq!(root.node, ids[0]);
    assert!(
        (root.feature.pos.x - 1.0).abs() < 1e-9,
        "{:?}",
        root.feature
    );
    assert!(root.feature.pos.y.abs() < 1e-12);
    assert!(hits.iter().all(|h| (h.feature.pos.x - -1.0).abs() > 0.5));
}

#[test]
fn finds_minimum() {
    let mut e = engine();
    let (_, vp) = plotted(&mut e, &["y = x^2 - 1"]);
    let hits = e.features_near(vp, Vec2::new(0.1, -0.9), 0.5);
    let min = hits
        .iter()
        .find(|h| h.feature.kind == FeatureKind::Minimum)
        .expect("minimum");
    assert!(min.feature.pos.x.abs() < 1e-4);
    assert!((min.feature.pos.y + 1.0).abs() < 1e-6);
}

#[test]
fn finds_intersection_of_two_curves() {
    let mut e = engine();
    let (ids, vp) = plotted(&mut e, &["y = x", "y = 2 - x"]);
    let hits = e.features_near(vp, Vec2::new(1.1, 0.9), 0.5);
    let ix = hits
        .iter()
        .find(|h| h.feature.kind == FeatureKind::Intersection)
        .expect("intersection");
    assert_eq!((ix.node, ix.other), (ids[0], Some(ids[1])));
    assert!((ix.feature.pos.x - 1.0).abs() < 1e-9);
    assert!((ix.feature.pos.y - 1.0).abs() < 1e-9);
}

#[test]
fn hits_are_sorted_and_within_radius() {
    let mut e = engine();
    let (_, vp) = plotted(&mut e, &["y = x^2 - 1"]);
    let hits = e.features_near(vp, Vec2::new(0.0, -0.5), 2.0);
    assert!(hits.len() >= 2);
    assert!(hits.windows(2).all(|w| w[0].distance <= w[1].distance));
    assert!(hits.iter().all(|h| h.distance <= 2.0));
}

#[test]
fn nothing_far_away() {
    let mut e = engine();
    let (_, vp) = plotted(&mut e, &["y = x^2 - 1"]);
    assert!(e.features_near(vp, Vec2::new(8.0, 8.0), 0.1).is_empty());
}

#[test]
fn unplotted_viewport_has_no_features() {
    let mut e = engine();
    plot(&mut e, "y = x^2 - 1");
    let vp = e.create_viewport();
    assert!(e.features_near(vp, Vec2::new(1.0, 0.0), 1.0).is_empty());
}

#[test]
fn non_explicit_plots_are_ignored() {
    let mut e = engine();
    let (_, vp) = plotted(&mut e, &["x^2 + y^2 = 1"]);
    assert!(e.features_near(vp, Vec2::new(1.0, 0.0), 0.5).is_empty());
}

#[test]
fn features_follow_variable_changes() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let (_, vp) = plotted(&mut e, &["y = x^2 - a"]);
    set(&mut e, a, "4");
    let hits = e.features_near(vp, Vec2::new(2.0, 0.0), 0.3);
    assert!(
        hits.iter()
            .any(|h| h.feature.kind == FeatureKind::Root && (h.feature.pos.x - 2.0).abs() < 1e-9)
    );
}

#[test]
fn node_features_lists_roots_extrema_and_intersections_by_x() {
    let mut e = engine();
    let (ids, _) = plotted(&mut e, &["y = x^2 - 1", "y = 3"]);
    let hits = e.node_features(ids[0], None);
    let roots: Vec<f64> = hits
        .iter()
        .filter(|h| h.feature.kind == FeatureKind::Root)
        .map(|h| h.feature.pos.x)
        .collect();
    assert_eq!(roots.len(), 2);
    assert!(roots[0] < roots[1], "sorted by x");
    assert!(hits.iter().any(|h| h.feature.kind == FeatureKind::Minimum));
    let crossings = |hits: &[crate::FeatureHit]| {
        hits.iter()
            .filter(|h| h.feature.kind == FeatureKind::Intersection)
            .count()
    };
    assert_eq!(crossings(&hits), 2);
    // The other curve lists the same intersections.
    assert_eq!(crossings(&e.node_features(ids[1], None)), 2);
}

#[test]
fn new_plot_nodes_get_auto_names() {
    let mut e = engine();
    let a = plot(&mut e, "y = x");
    let b = plot(&mut e, "x^2 + y^2 = 4");
    let c = plot(&mut e, "y < x");
    let name = |id| e.scene().get(id).and_then(|n| n.name.clone());
    assert_eq!(name(a).as_deref(), Some("Plot1"));
    assert_eq!(name(b).as_deref(), Some("Circle1"));
    assert_eq!(name(c).as_deref(), Some("Region1"));
}
