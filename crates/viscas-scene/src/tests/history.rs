//! Undo/redo of every operation, groups and coalescing.

use super::*;

/// Structural fingerprint of a scene (tree + contents).
fn snapshot(s: &Scene) -> String {
    let mut d = s.to_document();
    d.next_id = 0;
    save_document(&d).unwrap()
}

/// Run `op`, then check undo restores the before-state and redo the
/// after-state (twice, to catch asymmetric inverses).
fn check_undo_redo(s: &mut Scene, op: impl FnOnce(&mut Scene)) {
    let before = snapshot(s);
    op(s);
    let after = snapshot(s);
    assert_ne!(before, after, "operation changed nothing");
    for _ in 0..2 {
        assert!(s.undo());
        assert_eq!(snapshot(s), before);
        assert!(s.redo());
        assert_eq!(snapshot(s), after);
    }
}

#[test]
fn nothing_to_undo_initially() {
    let mut s = Scene::new();
    assert!(!s.can_undo());
    assert!(!s.can_redo());
    assert!(!s.undo());
    assert!(!s.redo());
}

#[test]
fn undo_add() {
    let mut s = Scene::new();
    check_undo_redo(&mut s, |s| {
        var(s, "a", "1");
    });
}

#[test]
fn undo_add_restores_same_id() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.undo();
    assert!(!s.contains(a));
    s.redo();
    assert!(s.contains(a));
    assert_eq!(s.lookup("a"), Some(a));
}

#[test]
fn undo_insert_in_middle() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    var(&mut s, "b", "2");
    check_undo_redo(&mut s, |s| {
        s.insert(None, 1, NodeContent::variable("c", "3")).unwrap();
    });
}

#[test]
fn undo_remove_subtree_restores_order_and_links() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    let b = s.add(Some(f), NodeContent::variable("b", "a")).unwrap();
    let g = s.add(Some(f), NodeContent::folder("G")).unwrap();
    s.add(Some(g), NodeContent::variable("c", "b")).unwrap();
    var(&mut s, "d", "2");
    check_undo_redo(&mut s, |s| s.remove(f).unwrap());
    s.undo();
    assert_eq!(s.index_in_parent(f), Some(1));
    assert_eq!(s.parent(b), Some(f));
    assert_eq!(s.graph().direct_upstream(b), vec![a]);
}

#[test]
fn undo_move() {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    var(&mut s, "x_0", "0");
    let a = var(&mut s, "a", "1");
    var(&mut s, "b", "2");
    check_undo_redo(&mut s, |s| s.move_node(a, Some(f), 0).unwrap());
    check_undo_redo(&mut s, |s| s.move_node(a, None, 0).unwrap());
}

#[test]
fn undo_reorder() {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    s.add(Some(f), NodeContent::variable("b", "1")).unwrap();
    s.add(Some(f), NodeContent::variable("c", "1")).unwrap();
    check_undo_redo(&mut s, |s| s.reorder(a, 2).unwrap());
}

#[test]
fn undo_rename() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    check_undo_redo(&mut s, |s| s.rename(a, Some("q")).unwrap());
}

#[test]
fn undo_visibility_lock_style_props_kind() {
    let mut s = Scene::new();
    let p = plot(&mut s, "y = x");
    check_undo_redo(&mut s, |s| s.set_visible(p, false).unwrap());
    check_undo_redo(&mut s, |s| s.set_locked(p, true).unwrap());
    s.seal_history();
    check_undo_redo(&mut s, |s| {
        s.set_style(
            p,
            Style {
                color: Some(Color::rgb(1, 2, 3)),
                ..Style::default()
            },
        )
        .unwrap()
    });
    s.seal_history();
    check_undo_redo(&mut s, |s| s.set_prop(p, "k", "v").unwrap());
    s.seal_history();
    check_undo_redo(&mut s, |s| s.remove_prop(p, "k").unwrap());
    check_undo_redo(&mut s, |s| {
        s.set_kind(
            p,
            NodeKind::Plot {
                relation: "y = 2x".into(),
            },
        )
        .unwrap()
    });
}

#[test]
fn undo_set_source_and_update() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    check_undo_redo(&mut s, |s| s.set_source(a, SourceField::Main, "2").unwrap());
    s.seal_history();
    check_undo_redo(&mut s, |s| s.update(a, |c| c.locked = true).unwrap());
}

#[test]
fn undo_duplicate() {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    check_undo_redo(&mut s, |s| {
        s.duplicate(f).unwrap();
    });
}

#[test]
fn undo_repoint() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    var(&mut s, "b", "1");
    let c = var(&mut s, "c", "a + 1");
    check_undo_redo(&mut s, |s| {
        s.repoint(c, "a", "b").unwrap();
    });
}

#[test]
fn undo_rename_refactor_is_one_step() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    var(&mut s, "b", "a + 1");
    var(&mut s, "c", "2a");
    check_undo_redo(&mut s, |s| {
        s.rename_refactor(a, "k").unwrap();
    });
}

#[test]
fn undo_remove_many_is_one_step() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    var(&mut s, "c", "3");
    check_undo_redo(&mut s, |s| s.remove_many(&[a, b]).unwrap());
}

#[test]
fn undo_restores_graph_and_dirty() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a");
    s.set_source(b, SourceField::Main, "2").unwrap();
    assert!(s.graph().direct_upstream(b).is_empty());
    s.take_dirty();
    s.undo();
    assert_eq!(s.graph().direct_upstream(b), vec![a]);
    assert!(s.is_dirty(b));
}

#[test]
fn new_edit_clears_redo() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    s.undo();
    assert!(s.can_redo());
    var(&mut s, "b", "1");
    assert!(!s.can_redo());
}

#[test]
fn labels() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    assert_eq!(s.undo_label(), Some("Add node"));
    s.rename(a, Some("b")).unwrap();
    assert_eq!(s.undo_label(), Some("Rename"));
    s.undo();
    assert_eq!(s.redo_label(), Some("Rename"));
}

#[test]
fn failed_operations_record_nothing() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    var(&mut s, "b", "1");
    let depth = s.undo_depth();
    assert!(s.rename(a, Some("b")).is_err());
    assert!(s.move_node(a, Some(a), 0).is_err());
    assert!(s.remove(NodeId(42)).is_err());
    assert_eq!(s.undo_depth(), depth);
}

#[test]
fn many_undos_back_to_empty() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    s.move_node(a, Some(f), 0).unwrap();
    s.rename(a, Some("b")).unwrap();
    s.set_source(a, SourceField::Main, "3").unwrap();
    s.remove(f).unwrap();
    while s.undo() {}
    assert!(s.is_empty());
    while s.redo() {}
    assert_eq!(s.len(), 0);
    assert!(!s.contains(a));
    s.undo();
    assert_eq!(s.lookup("b"), Some(a));
    assert_eq!(main_src(&s, a), "3");
}

// ---------------------------------------------------------------------------
// Coalescing
// ---------------------------------------------------------------------------

#[test]
fn slider_drag_coalesces_into_one_step() {
    let mut s = Scene::new();
    let a = s
        .add(None, NodeContent::slider("a", "0", "0", "10"))
        .unwrap();
    let depth = s.undo_depth();
    for v in 1..=20 {
        s.set_source(a, SourceField::Main, &format!("{}", v as f64 / 2.0))
            .unwrap();
    }
    assert_eq!(s.undo_depth(), depth + 1);
    assert_eq!(main_src(&s, a), "10");
    s.undo();
    assert_eq!(main_src(&s, a), "0");
    s.redo();
    assert_eq!(main_src(&s, a), "10");
}

#[test]
fn seal_starts_a_new_step() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    s.set_source(a, SourceField::Main, "1").unwrap();
    s.set_source(a, SourceField::Main, "2").unwrap();
    s.seal_history();
    s.set_source(a, SourceField::Main, "3").unwrap();
    s.undo();
    assert_eq!(main_src(&s, a), "2");
    s.undo();
    assert_eq!(main_src(&s, a), "0");
}

#[test]
fn different_fields_do_not_coalesce() {
    let mut s = Scene::new();
    let a = s
        .add(None, NodeContent::slider("a", "0", "0", "10"))
        .unwrap();
    s.set_source(a, SourceField::Main, "1").unwrap();
    s.set_source(a, SourceField::Max, "20").unwrap();
    s.undo();
    assert_eq!(main_src(&s, a), "1");
}

#[test]
fn different_nodes_do_not_coalesce() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    let b = var(&mut s, "b", "0");
    s.set_source(a, SourceField::Main, "1").unwrap();
    s.set_source(b, SourceField::Main, "1").unwrap();
    s.undo();
    assert_eq!(main_src(&s, a), "1");
    assert_eq!(main_src(&s, b), "0");
}

#[test]
fn non_coalescing_edit_breaks_the_run() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    s.set_source(a, SourceField::Main, "1").unwrap();
    s.set_visible(a, false).unwrap();
    s.set_source(a, SourceField::Main, "2").unwrap();
    s.undo();
    assert_eq!(main_src(&s, a), "1");
}

#[test]
fn undo_breaks_coalescing() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    s.set_source(a, SourceField::Main, "1").unwrap();
    s.seal_history();
    s.set_source(a, SourceField::Main, "2").unwrap();
    s.undo();
    s.set_source(a, SourceField::Main, "5").unwrap();
    s.undo();
    assert_eq!(main_src(&s, a), "1");
}

#[test]
fn drag_back_to_start_cancels_the_step() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    let depth = s.undo_depth();
    s.set_source(a, SourceField::Main, "1").unwrap();
    s.set_source(a, SourceField::Main, "0").unwrap();
    assert_eq!(s.undo_depth(), depth);
}

#[test]
fn style_and_prop_edits_coalesce_per_key() {
    let mut s = Scene::new();
    let p = plot(&mut s, "y = x");
    let depth = s.undo_depth();
    for w in 1..5 {
        s.set_style(
            p,
            Style {
                line_width: w as f64,
                ..Style::default()
            },
        )
        .unwrap();
    }
    assert_eq!(s.undo_depth(), depth + 1);
    s.set_prop(p, "speed", 1.0).unwrap();
    s.set_prop(p, "speed", 2.0).unwrap();
    assert_eq!(s.undo_depth(), depth + 2);
    s.set_prop(p, "other", 1.0).unwrap();
    assert_eq!(s.undo_depth(), depth + 3);
}

#[test]
fn update_coalesced_custom_key() {
    let mut s = Scene::new();
    let v = s.add(None, NodeContent::viewport("V")).unwrap();
    let depth = s.undo_depth();
    for i in 0..10 {
        s.update_coalesced(v, "pan", |c| {
            if let NodeKind::Viewport { bounds, .. } = &mut c.kind {
                bounds.x_min = -10.0 + i as f64;
            }
        })
        .unwrap();
    }
    assert_eq!(s.undo_depth(), depth + 1);
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

#[test]
fn group_is_one_step() {
    let mut s = Scene::new();
    let before = snapshot(&s);
    s.begin_group("Build");
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    s.rename(a, Some("b")).unwrap();
    s.end_group();
    assert_eq!(s.undo_depth(), 1);
    assert_eq!(s.undo_label(), Some("Build"));
    let after = snapshot(&s);
    s.undo();
    assert_eq!(snapshot(&s), before);
    s.redo();
    assert_eq!(snapshot(&s), after);
}

#[test]
fn nested_groups_keep_outer_label() {
    let mut s = Scene::new();
    s.begin_group("Outer");
    var(&mut s, "a", "1");
    s.begin_group("Inner");
    var(&mut s, "b", "1");
    s.end_group();
    assert!(s.in_group());
    var(&mut s, "c", "1");
    s.end_group();
    assert!(!s.in_group());
    assert_eq!(s.undo_depth(), 1);
    assert_eq!(s.undo_label(), Some("Outer"));
    s.undo();
    assert!(s.is_empty());
}

#[test]
fn empty_group_records_nothing() {
    let mut s = Scene::new();
    s.begin_group("Nothing");
    s.end_group();
    assert_eq!(s.undo_depth(), 0);
    s.end_group(); // unbalanced end is harmless
}

#[test]
fn coalescing_inside_group() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    s.begin_group("Drag");
    for i in 1..10 {
        s.set_source(a, SourceField::Main, &i.to_string()).unwrap();
    }
    s.end_group();
    s.undo();
    assert_eq!(main_src(&s, a), "0");
    s.redo();
    assert_eq!(main_src(&s, a), "9");
}

#[test]
fn undo_closes_open_group() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    s.begin_group("G");
    var(&mut s, "b", "1");
    assert!(s.can_undo());
    s.undo();
    assert!(!s.in_group());
    assert_eq!(s.len(), 1);
}

#[test]
fn transaction_helper() {
    let mut s = Scene::new();
    let ids = s.transaction("Two", |s| (var(s, "a", "1"), var(s, "b", "a")));
    assert_eq!(s.undo_depth(), 1);
    s.undo();
    assert!(!s.contains(ids.0) && !s.contains(ids.1));
}

#[test]
fn group_with_remove_and_move_undoes_in_reverse() {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a");
    let before = snapshot(&s);
    s.transaction("Mix", |s| {
        s.move_node(a, Some(f), 0).unwrap();
        s.move_node(b, Some(f), 1).unwrap();
        s.remove(a).unwrap();
        s.rename(b, Some("q")).unwrap();
        s.remove(f).unwrap();
    });
    assert!(s.is_empty());
    s.undo();
    assert_eq!(snapshot(&s), before);
}

#[test]
fn history_limit_drops_oldest() {
    let mut s = Scene::new();
    s.set_history_limit(3);
    for i in 0..10 {
        var(&mut s, &format!("v_{i}"), "1");
    }
    assert_eq!(s.undo_depth(), 3);
    while s.undo() {}
    assert_eq!(s.len(), 7);
}

#[test]
fn clear_history() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    s.clear_history();
    assert!(!s.can_undo());
    assert_eq!(s.len(), 1);
}

#[test]
fn history_changed_events() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    assert!(s.drain_events().contains(&SceneEvent::HistoryChanged));
    s.undo();
    assert!(s.drain_events().contains(&SceneEvent::HistoryChanged));
}

#[test]
fn undo_emits_inverse_events() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.drain_events();
    s.undo();
    assert!(s.drain_events().contains(&SceneEvent::NodeRemoved(a)));
    s.redo();
    assert!(s.drain_events().contains(&SceneEvent::NodeAdded(a)));
}
