//! Tree operations, names, selection and events.

use super::*;

fn folder(s: &mut Scene, parent: Option<NodeId>, name: &str) -> NodeId {
    s.add(parent, NodeContent::folder(name)).unwrap()
}

#[test]
fn new_scene_is_empty() {
    let s = Scene::new();
    assert!(s.is_empty());
    assert_eq!(s.len(), 0);
    assert!(s.roots().is_empty());
    assert_eq!(s.iter().count(), 0);
}

#[test]
fn add_appends_top_level_in_order() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    assert_eq!(s.roots(), &[a, b]);
    assert_eq!(s.len(), 2);
    assert_eq!(s.parent(a), None);
}

#[test]
fn ids_are_unique_and_increasing() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    assert!(b > a);
    s.remove(b).unwrap();
    let c = var(&mut s, "c", "3");
    assert!(c > b, "ids are never reused");
}

#[test]
fn insert_at_index() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    let c = s.insert(None, 1, NodeContent::variable("c", "3")).unwrap();
    assert_eq!(s.roots(), &[a, c, b]);
    let d = s.insert(None, 0, NodeContent::variable("d", "4")).unwrap();
    assert_eq!(s.roots(), &[d, a, c, b]);
}

#[test]
fn insert_index_out_of_range() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    let err = s
        .insert(None, 5, NodeContent::variable("c", "3"))
        .unwrap_err();
    assert_eq!(err, SceneError::IndexOutOfRange { index: 5, len: 1 });
    assert_eq!(s.len(), 1);
}

#[test]
fn add_under_missing_parent_fails() {
    let mut s = Scene::new();
    let err = s
        .add(Some(NodeId(99)), NodeContent::folder("x"))
        .unwrap_err();
    assert_eq!(err, SceneError::NoSuchNode(NodeId(99)));
}

#[test]
fn children_and_parent_links() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "Geometry");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let b = s.add(Some(f), NodeContent::variable("b", "2")).unwrap();
    assert_eq!(s.children(Some(f)), &[a, b]);
    assert_eq!(s.parent(a), Some(f));
    assert_eq!(s.depth(a), 1);
    assert_eq!(s.index_in_parent(b), Some(1));
    assert!(s.is_ancestor(f, b));
    assert!(!s.is_ancestor(b, f));
}

#[test]
fn depth_first_preorder() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let g = folder(&mut s, Some(f), "G");
    let b = s.add(Some(g), NodeContent::variable("b", "2")).unwrap();
    let c = var(&mut s, "c", "3");
    assert_eq!(s.dfs_ids(), vec![f, a, g, b, c]);
    assert_eq!(s.descendants(f), vec![a, g, b]);
    let sub: Vec<NodeId> = s.iter_subtree(g).map(|n| n.id).collect();
    assert_eq!(sub, vec![g, b]);
}

#[test]
fn remove_deletes_subtree() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let g = folder(&mut s, Some(f), "G");
    let b = s.add(Some(g), NodeContent::variable("b", "2")).unwrap();
    let c = var(&mut s, "c", "3");
    s.remove(f).unwrap();
    assert_eq!(s.len(), 1);
    for id in [f, a, g, b] {
        assert!(!s.contains(id));
    }
    assert_eq!(s.roots(), &[c]);
}

#[test]
fn remove_missing_node_fails() {
    let mut s = Scene::new();
    assert_eq!(s.remove(NodeId(7)), Err(SceneError::NoSuchNode(NodeId(7))));
}

#[test]
fn remove_many_skips_nested() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let c = var(&mut s, "c", "3");
    let depth = s.undo_depth();
    s.remove_many(&[f, a, c]).unwrap();
    assert!(s.is_empty());
    assert_eq!(s.undo_depth(), depth + 1, "one undo step");
}

#[test]
fn move_into_folder() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = var(&mut s, "a", "1");
    s.move_node(a, Some(f), 0).unwrap();
    assert_eq!(s.roots(), &[f]);
    assert_eq!(s.children(Some(f)), &[a]);
    assert_eq!(s.parent(a), Some(f));
}

#[test]
fn move_out_to_top_level() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    s.move_node(a, None, 0).unwrap();
    assert_eq!(s.roots(), &[a, f]);
    assert_eq!(s.parent(a), None);
    assert!(s.children(Some(f)).is_empty());
}

#[test]
fn move_into_self_is_refused() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    assert_eq!(
        s.move_node(f, Some(f), 0),
        Err(SceneError::WouldCycle { node: f, parent: f })
    );
}

#[test]
fn move_into_descendant_is_refused() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let g = folder(&mut s, Some(f), "G");
    let h = folder(&mut s, Some(g), "H");
    assert_eq!(
        s.move_node(f, Some(h), 0),
        Err(SceneError::WouldCycle { node: f, parent: h })
    );
    assert_eq!(s.parent(g), Some(f));
    assert_eq!(s.roots(), &[f]);
}

#[test]
fn move_sibling_into_sibling_is_fine() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let g = folder(&mut s, None, "G");
    s.move_node(g, Some(f), 0).unwrap();
    assert_eq!(s.parent(g), Some(f));
}

#[test]
fn move_index_counts_without_the_node() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    let c = var(&mut s, "c", "3");
    // Moving `a` to the end: index 2 among [b, c].
    s.move_node(a, None, 2).unwrap();
    assert_eq!(s.roots(), &[b, c, a]);
    assert_eq!(
        s.move_node(a, None, 3),
        Err(SceneError::IndexOutOfRange { index: 3, len: 2 })
    );
}

#[test]
fn reorder_siblings() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let b = s.add(Some(f), NodeContent::variable("b", "2")).unwrap();
    let c = s.add(Some(f), NodeContent::variable("c", "3")).unwrap();
    s.reorder(c, 0).unwrap();
    assert_eq!(s.children(Some(f)), &[c, a, b]);
    s.reorder(c, 1).unwrap();
    assert_eq!(s.children(Some(f)), &[a, c, b]);
}

#[test]
fn moving_keeps_subtree() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let g = folder(&mut s, None, "G");
    s.move_node(f, Some(g), 0).unwrap();
    assert_eq!(s.dfs_ids(), vec![g, f, a]);
}

#[test]
fn rename_defining_node() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.rename(a, Some("b")).unwrap();
    assert_eq!(s.get(a).unwrap().name.as_deref(), Some("b"));
    assert_eq!(s.lookup("b"), Some(a));
    assert_eq!(s.lookup("a"), None);
}

#[test]
fn rename_rejects_duplicate() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    var(&mut s, "b", "2");
    assert_eq!(
        s.rename(a, Some("b")),
        Err(SceneError::DuplicateName("b".into()))
    );
    assert_eq!(s.get(a).unwrap().name.as_deref(), Some("a"));
}

#[test]
fn rename_to_same_name_is_noop() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let depth = s.undo_depth();
    s.rename(a, Some("a")).unwrap();
    assert_eq!(s.undo_depth(), depth);
}

#[test]
fn rename_rejects_invalid_identifier() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    for bad in ["1a", "a b", "for", "a+b", "", "x_{1}", "(a)"] {
        if bad.is_empty() {
            continue;
        }
        assert_eq!(
            s.rename(a, Some(bad)),
            Err(SceneError::InvalidName(bad.into())),
            "{bad}"
        );
    }
}

#[test]
fn add_rejects_duplicate_and_invalid() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    assert_eq!(
        s.add(None, NodeContent::variable("a", "2")),
        Err(SceneError::DuplicateName("a".into()))
    );
    assert_eq!(
        s.add(None, NodeContent::function("g", &["x", "x"], "x")),
        Err(SceneError::InvalidName("x".into()))
    );
    assert_eq!(
        s.add(None, NodeContent::variable("let", "2")),
        Err(SceneError::InvalidName("let".into()))
    );
    assert_eq!(s.len(), 1);
}

#[test]
fn labels_of_non_defining_nodes_are_free_text() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "My shapes");
    let g = folder(&mut s, None, "My shapes");
    let p = plot(&mut s, "y = x");
    s.rename(p, Some("a")).unwrap();
    // A plot labelled `a` does not block a variable `a`.
    var(&mut s, "a", "1");
    assert_ne!(f, g);
}

#[test]
fn empty_name_clears() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.rename(a, Some("  ")).unwrap();
    assert_eq!(s.get(a).unwrap().name, None);
    s.rename(a, None).unwrap();
    assert_eq!(s.get(a).unwrap().name, None);
}

#[test]
fn regression_params_must_be_unique_names() {
    let mut s = Scene::new();
    var(&mut s, "m", "1");
    assert_eq!(
        s.add(
            None,
            NodeContent::regression("y_1 ~ m x_1 + b", &["m", "b"])
        ),
        Err(SceneError::DuplicateName("m".into()))
    );
    let r = s
        .add(
            None,
            NodeContent::regression("y_1 ~ k x_1 + b", &["k", "b"]),
        )
        .unwrap();
    assert_eq!(s.lookup("k"), Some(r));
    assert_eq!(
        s.add(None, NodeContent::variable("b", "2")),
        Err(SceneError::DuplicateName("b".into()))
    );
}

#[test]
fn unique_name_generation() {
    let mut s = Scene::new();
    assert_eq!(s.unique_name("a"), "a");
    var(&mut s, "a", "1");
    assert_eq!(s.unique_name("a"), "a_2");
    var(&mut s, "a_2", "1");
    assert_eq!(s.unique_name("a"), "a_3");
    var(&mut s, "x_1", "1");
    assert_eq!(s.unique_name("x_1"), "x_12");
    assert_eq!(s.unique_name("not valid"), "n");
}

#[test]
fn lookup_by_label_falls_back() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "Shapes");
    assert_eq!(s.lookup("Shapes"), Some(f));
    assert_eq!(s.lookup("nothing"), None);
}

#[test]
fn find_and_find_kind() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let p = plot(&mut s, "y = a x");
    let f = func(&mut s, "f", &["x"], "x");
    assert_eq!(s.find_kind(KindTag::Plot), vec![p]);
    assert_eq!(s.find(|n| n.kind.defines_by_name()), vec![a, f]);
}

#[test]
fn set_visible_locked_style_props() {
    let mut s = Scene::new();
    let p = plot(&mut s, "y = x");
    s.set_visible(p, false).unwrap();
    s.set_locked(p, true).unwrap();
    let style = Style {
        color: Some(Color::rgb(255, 0, 0)),
        line_width: 3.0,
        ..Style::default()
    };
    s.set_style(p, style.clone()).unwrap();
    s.set_prop(p, "draggable", true).unwrap();
    let n = s.get(p).unwrap();
    assert!(!n.visible);
    assert!(n.locked);
    assert_eq!(n.style, style);
    assert_eq!(n.props.get("draggable"), Some(&PropValue::Bool(true)));
    s.remove_prop(p, "draggable").unwrap();
    assert!(s.get(p).unwrap().props.is_empty());
}

#[test]
fn set_source_fields() {
    let mut s = Scene::new();
    let sl = s
        .add(None, NodeContent::slider("a", "3", "0", "10"))
        .unwrap();
    s.set_source(sl, SourceField::Max, "20").unwrap();
    s.set_source(sl, SourceField::Step, "0.5").unwrap();
    match &s.get(sl).unwrap().kind {
        NodeKind::Slider { max, step, .. } => {
            assert_eq!(max, "20");
            assert_eq!(step, "0.5");
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(
        s.set_source(sl, SourceField::Formula { column: 0 }, "x"),
        Err(SceneError::NoSuchField(SourceField::Formula { column: 0 }))
    );
}

#[test]
fn set_kind_changes_type() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "3");
    s.set_kind(a, NodeKind::slider("3")).unwrap();
    assert_eq!(s.get(a).unwrap().kind.tag(), KindTag::Slider);
    assert_eq!(s.lookup("a"), Some(a));
}

#[test]
fn duplicate_subtree_with_fresh_names() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let b = s.add(Some(f), NodeContent::variable("b", "a + 1")).unwrap();
    let c = var(&mut s, "c", "0");
    let f2 = s.duplicate(f).unwrap();
    assert_eq!(s.roots(), &[f, f2, c]);
    let kids = s.children(Some(f2)).to_vec();
    assert_eq!(kids.len(), 2);
    assert!(!kids.contains(&a) && !kids.contains(&b));
    assert_eq!(titles(&s, &kids), vec!["a_2", "b_2"]);
    assert_eq!(s.parent(kids[0]), Some(f2));
    // The copy of `b` still references the original `a`.
    assert_eq!(s.graph().direct_upstream(kids[1]), vec![a]);
    assert!(s.undo());
    assert_eq!(s.len(), 4);
}

#[test]
fn selection_basics() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    s.drain_events();
    s.select(a);
    assert_eq!(s.selection(), &[a]);
    s.add_to_selection(b);
    assert_eq!(s.selection(), &[a, b]);
    assert_eq!(s.primary_selection(), Some(b));
    s.add_to_selection(a);
    assert_eq!(s.selection(), &[b, a], "re-adding makes it primary");
    s.toggle_selection(b);
    assert_eq!(s.selection(), &[a]);
    s.toggle_selection(b);
    assert!(s.is_selected(b));
    s.clear_selection();
    assert!(s.selection().is_empty());
    let ev = s.drain_events();
    assert!(ev.iter().all(|e| *e == SceneEvent::SelectionChanged));
    assert_eq!(ev.len(), 6);
}

#[test]
fn selection_ignores_unknown_ids_and_duplicates() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.set_selection([a, NodeId(77), a]);
    assert_eq!(s.selection(), &[a]);
    s.select_all();
    assert_eq!(s.selection(), &[a]);
}

#[test]
fn removing_selected_nodes_prunes_selection() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = s.add(Some(f), NodeContent::variable("a", "1")).unwrap();
    let b = var(&mut s, "b", "1");
    s.set_selection([a, b]);
    s.remove(f).unwrap();
    assert_eq!(s.selection(), &[b]);
}

#[test]
fn events_for_add_remove_move_change() {
    let mut s = Scene::new();
    let f = folder(&mut s, None, "F");
    let a = var(&mut s, "a", "1");
    let ev = s.drain_events();
    assert!(ev.contains(&SceneEvent::NodeAdded(f)));
    assert!(ev.contains(&SceneEvent::NodeAdded(a)));
    s.move_node(a, Some(f), 0).unwrap();
    assert!(s.drain_events().contains(&SceneEvent::Moved {
        id: a,
        old_parent: None,
        new_parent: Some(f)
    }));
    s.rename(a, Some("b")).unwrap();
    assert!(s.drain_events().contains(&SceneEvent::Changed {
        id: a,
        what: Change::Name
    }));
    s.set_visible(a, false).unwrap();
    let ev = s.drain_events();
    assert!(ev.contains(&SceneEvent::Changed {
        id: a,
        what: Change::Visible
    }));
    assert!(!ev.iter().any(|e| matches!(
        e,
        SceneEvent::Changed {
            what: Change::Name,
            ..
        }
    )));
    s.remove(f).unwrap();
    let ev = s.drain_events();
    assert!(ev.contains(&SceneEvent::NodeRemoved(f)));
    assert!(ev.contains(&SceneEvent::NodeRemoved(a)));
    assert!(!s.has_events());
}

#[test]
fn graph_changed_event_only_when_graph_changes() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a");
    s.drain_events();
    s.set_style(
        b,
        Style {
            line_width: 5.0,
            ..Style::default()
        },
    )
    .unwrap();
    assert!(!s.drain_events().contains(&SceneEvent::GraphChanged));
    s.set_source(b, SourceField::Main, "2").unwrap();
    assert!(s.drain_events().contains(&SceneEvent::GraphChanged));
}

#[test]
fn node_title_and_defined_name() {
    let mut s = Scene::new();
    let p = plot(&mut s, "y = x");
    let a = var(&mut s, "a", "1");
    assert_eq!(s.get(p).unwrap().title(), "plot");
    assert_eq!(s.get(p).unwrap().defined_name(), None);
    assert_eq!(s.get(a).unwrap().defined_name(), Some("a"));
}

#[test]
fn every_kind_can_be_added() {
    let mut s = Scene::new();
    let contents = vec![
        NodeContent::folder("F"),
        NodeContent::variable("a", "3"),
        NodeContent::function("f", &["x"], "x^2"),
        NodeContent::plot("y = x"),
        NodeContent::point("P", "(1, 2)"),
        NodeContent::slider("s", "1", "0", "5"),
        NodeContent::list("L", "[1..10]"),
        NodeContent::table("T", vec![Column::new("x_1", &["1", "2"])]),
        NodeContent::script(ScriptLanguage::Viscas, "let q = 1"),
        NodeContent::script(ScriptLanguage::Rhai, "let q = 1;"),
        NodeContent::note("hello"),
        NodeContent::viewport("Main"),
        NodeContent::action("a -> a + 1"),
        NodeContent::ticker("s -> s + 1", 100),
        NodeContent::regression("y_1 ~ m x_1 + c", &["m", "c"]),
    ];
    let mut tags = std::collections::BTreeSet::new();
    for c in contents {
        let id = s.add(None, c).unwrap();
        tags.insert(s.get(id).unwrap().kind.tag());
    }
    assert_eq!(tags.len(), KindTag::ALL.len());
}
