//! Regression tests for the PR #9 review findings.

use std::time::Instant;

use super::*;

// 1–3: duplicate() rewrites everything it renames ---------------------------

#[test]
fn duplicate_rewrites_regression_params_in_source() {
    let mut s = Scene::new();
    let r = s
        .add(
            None,
            NodeContent::regression("y_1 ~ m x_1 + b", &["m", "b"]),
        )
        .unwrap();
    let r2 = s.duplicate(r).unwrap();
    match &s.get(r2).unwrap().kind {
        NodeKind::Regression { source, params } => {
            assert_eq!(params, &vec!["m_2".to_string(), "b_2".to_string()]);
            assert_eq!(source, "y_1 ~ m_2 x_1 + b_2");
        }
        k => panic!("{k:?}"),
    }
    assert!(s.diagnostics(r2).iter().all(|d| !d.is_error()));
}

#[test]
fn duplicate_renames_script_functions_and_their_uses() {
    let mut s = Scene::new();
    let sc = s
        .add(
            None,
            NodeContent::script(
                ScriptLanguage::Viscas,
                "fn h(u) {\n    return h(u - 1)\n}\nh(2)",
            ),
        )
        .unwrap();
    let copy = s.duplicate(sc).unwrap();
    assert_eq!(
        main_src(&s, copy),
        "fn h_2(u) {\n    return h_2(u - 1)\n}\nh_2(2)"
    );
    assert_eq!(s.defines(copy), &["h_2".to_string()]);
    assert!(s.diagnostics(copy).is_empty());
    assert!(s.diagnostics(sc).is_empty(), "no duplicate definition");
}

#[test]
fn duplicate_recursive_function_calls_its_copy() {
    let mut s = Scene::new();
    let f = func(&mut s, "f", &["n"], "{n <= 1: 1, n f(n - 1)}");
    let u = var(&mut s, "u", "f(3)");
    let f2 = s.duplicate(f).unwrap();
    assert_eq!(s.get(f2).unwrap().name.as_deref(), Some("f_2"));
    assert_eq!(main_src(&s, f2), "{n <= 1: 1, n f_2(n - 1)}");
    assert_eq!(main_src(&s, u), "f(3)", "outside references untouched");
    assert_eq!(s.graph().direct_upstream(u), vec![f]);
}

// 4: name capture ------------------------------------------------------------

#[test]
fn repoint_into_a_parameter_is_captured() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    let g = func(&mut s, "g", &["b"], "a * b");
    assert_eq!(
        s.repoint(g, "a", "b"),
        Err(SceneError::NameCapture {
            node: g,
            name: "b".into()
        })
    );
    assert_eq!(main_src(&s, g), "a * b");
}

#[test]
fn repoint_into_a_local_binder_is_captured() {
    let mut s = Scene::new();
    let c = var(&mut s, "c", "∑(k = 1, 3) a k");
    assert!(matches!(
        s.repoint(c, "a", "k"),
        Err(SceneError::NameCapture { .. })
    ));
}

#[test]
fn rename_refactor_refuses_capture_in_dependent() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let c = var(&mut s, "c", "∑(k = 1, 3) a k");
    let depth = s.undo_depth();
    assert_eq!(
        s.rename_refactor(a, "k"),
        Err(SceneError::NameCapture {
            node: c,
            name: "k".into()
        })
    );
    assert_eq!(s.undo_depth(), depth);
    assert_eq!(s.lookup("a"), Some(a));
    assert_eq!(main_src(&s, c), "∑(k = 1, 3) a k");
}

#[test]
fn rename_refactor_refuses_rebinding_builtins() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let p = plot(&mut s, "y = sin x + a");
    assert_eq!(
        s.rename_refactor(a, "sin"),
        Err(SceneError::NameCapture {
            node: p,
            name: "sin".into()
        })
    );
    // An unused builtin name is fine.
    s.rename_refactor(a, "cos").unwrap();
}

// 5: ids ---------------------------------------------------------------------

#[test]
fn document_id_overflow_is_an_error() {
    let text = format!("(version: 1, nodes: [(id: {}, kind: Folder)])", u64::MAX);
    assert_eq!(
        Scene::load_from_str(&text).unwrap_err(),
        DocumentError::IdOverflow(NodeId(u64::MAX))
    );
}

#[test]
fn id_exhaustion_is_an_error() {
    let mut s = Scene::new();
    s.next_id = u64::MAX;
    assert_eq!(
        s.add(None, NodeContent::folder("F")),
        Err(SceneError::IdsExhausted)
    );
    assert!(s.is_empty());
}

#[test]
fn raw_insert_refuses_existing_ids() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let clash = s.get(a).unwrap().clone();
    assert_eq!(
        s.raw_insert(vec![clash], None, 0),
        Err(SceneError::IdInUse(a))
    );
    assert_eq!(s.len(), 1);
    assert_eq!(main_src(&s, a), "1");
}

// 6: duplicates dirty each other ---------------------------------------------

#[test]
fn fixing_a_duplicate_dirties_the_other_definer() {
    let mut s = Scene::load_from_str(
        r#"(version: 1, nodes: [
            (id: 1, name: "a", kind: Variable(expr: "1")),
            (id: 2, name: "a", kind: Variable(expr: "2")),
            (id: 3, name: "u", kind: Variable(expr: "a")),
        ])"#,
    )
    .unwrap();
    s.take_dirty();
    s.rename(NodeId(2), Some("b")).unwrap();
    let dirty = s.take_dirty();
    assert!(dirty.contains(&NodeId(1)), "{dirty:?}");
    assert!(dirty.contains(&NodeId(2)));
    assert!(dirty.contains(&NodeId(3)));
}

// 7: script functions take part in name checks -------------------------------

#[test]
fn script_function_names_must_be_free() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    assert_eq!(
        s.add(
            None,
            NodeContent::script(ScriptLanguage::Viscas, "fn a(x) { return x }")
        ),
        Err(SceneError::DuplicateName("a".into()))
    );
    let sc = s
        .add(
            None,
            NodeContent::script(ScriptLanguage::Viscas, "fn h(x) { return x }"),
        )
        .unwrap();
    assert_eq!(
        s.add(None, NodeContent::variable("h", "1")),
        Err(SceneError::DuplicateName("h".into()))
    );
    assert_eq!(
        s.set_source(sc, SourceField::Main, "fn a(x) { return x }"),
        Err(SceneError::DuplicateName("a".into()))
    );
    // Editing the body of an existing definition stays allowed.
    s.set_source(sc, SourceField::Main, "fn h(x) { return 2x }")
        .unwrap();
}

// 8: local recursive functions in scripts -------------------------------------

#[test]
fn script_local_function_may_recurse() {
    let c = NodeContent::script(ScriptLanguage::Viscas, "g(x) = g(x - 1) + k\ng(3)");
    assert_eq!(refs(&c), vec!["k".to_string()]);
}

// 9: repoint_edge rewrites only that edge -------------------------------------

#[test]
fn repoint_edge_only_touches_occurrences_of_that_edge() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    var(&mut s, "b", "2");
    let q = var(&mut s, "q", "3");
    let c = var(&mut s, "c", "ab + b + a");
    assert_eq!(s.repoint_edge(c, a, q).unwrap(), 2);
    assert_eq!(main_src(&s, c), "q b + b + q");
}

// 10: stale spans --------------------------------------------------------------

#[test]
fn repoint_detects_source_desync() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    let c = var(&mut s, "c", "a + 1");
    // Corrupt the source behind the cache's back.
    if let NodeKind::Variable { expr } = &mut s.nodes.get_mut(&c).unwrap().kind {
        *expr = "zz + 1".into();
    }
    assert_eq!(
        s.repoint(c, "a", "q"),
        Err(SceneError::SourceMismatch {
            node: c,
            field: SourceField::Main
        })
    );
    assert_eq!(main_src(&s, c), "zz + 1");
}

// 11: document validation -----------------------------------------------------

#[test]
fn document_with_invalid_names_is_rejected() {
    let text = r#"(version: 1, nodes: [(id: 1, name: "1a", kind: Variable(expr: "1"))])"#;
    assert!(matches!(
        Scene::load_from_str(text).unwrap_err(),
        DocumentError::InvalidContent { id: NodeId(1), .. }
    ));
    let text = r#"(version: 1, nodes: [(id: 1, name: "f", kind: Function(params: ["x", "x"], body: "x"))])"#;
    assert!(matches!(
        Scene::load_from_str(text).unwrap_err(),
        DocumentError::InvalidContent { .. }
    ));
}

#[test]
fn document_names_are_normalized() {
    let text = r#"(version: 1, nodes: [(id: 1, name: "  ", kind: Folder)])"#;
    let s = Scene::load_from_str(text).unwrap();
    assert_eq!(s.get(NodeId(1)).unwrap().name, None);
}

// 12: unknown fields round-trip -----------------------------------------------

#[test]
fn unknown_fields_round_trip() {
    let text = r#"(
        version: 1,
        nodes: [
            (id: 1, name: "a", kind: Variable(expr: "1"), glow: (radius: 2.5, tint: "red")),
        ],
        theme: Dark(contrast: 3),
    )"#;
    let mut s = Scene::load_from_str(text).unwrap();
    let a = NodeId(1);
    assert_eq!(
        s.get(a).unwrap().extra.get("glow").map(String::as_str),
        Some("(radius: 2.5, tint: \"red\")")
    );
    // Survives edits and undo.
    s.set_source(a, SourceField::Main, "2").unwrap();
    s.undo();
    let saved = s.save_to_string().unwrap();
    assert!(
        saved.contains("glow: (radius: 2.5, tint: \"red\")"),
        "{saved}"
    );
    assert!(saved.contains("theme: Dark(contrast: 3)"), "{saved}");
    let t = Scene::load_from_str(&saved).unwrap();
    assert_eq!(t.to_document(), s.to_document());
}

// 13: performance ----------------------------------------------------------------

#[test]
fn two_thousand_single_inserts_are_fast() {
    let start = Instant::now();
    let mut s = Scene::new();
    var(&mut s, "v_0", "1");
    for i in 1..2000 {
        var(&mut s, &format!("v_{i}"), &format!("v_{} + 1", i - 1));
    }
    let first = s.lookup("v_0").unwrap();
    let last = s.lookup("v_1999").unwrap();
    assert_eq!(s.downstream(first).len(), 1999);
    assert_eq!(s.take_dirty().len(), 2000);
    assert_eq!(*s.topo_order().last().unwrap(), last);
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f64() < 2.0, "took {elapsed:?}");
}

#[test]
fn moves_do_not_invalidate_the_graph() {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a");
    let topo = s.topo_order().to_vec();
    s.drain_events();
    s.move_node(b, Some(f), 0).unwrap();
    s.reorder(b, 0).unwrap();
    assert!(
        !s.drain_events()
            .iter()
            .any(|e| matches!(e, SceneEvent::GraphChanged { .. }))
    );
    assert_eq!(s.topo_order(), topo.as_slice());
}

// 14: GraphChanged payload -----------------------------------------------------

#[test]
fn graph_changed_lists_touched_nodes() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    assert!(
        s.drain_events()
            .contains(&SceneEvent::GraphChanged { touched: vec![a] })
    );
    s.set_builtins(std::sync::Arc::new(DefaultBuiltins));
    assert!(
        s.drain_events()
            .contains(&SceneEvent::GraphChanged { touched: vec![] })
    );
}

// Nits ------------------------------------------------------------------------

#[test]
fn lookup_is_none_for_duplicates() {
    let s = Scene::load_from_str(
        r#"(version: 1, nodes: [
            (id: 1, name: "a", kind: Variable(expr: "1")),
            (id: 2, name: "a", kind: Variable(expr: "2")),
        ])"#,
    )
    .unwrap();
    assert_eq!(s.lookup("a"), None);
    assert_eq!(s.definers("a"), vec![NodeId(1), NodeId(2)]);
}

#[test]
fn rename_refactor_reports_rewrites() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a + a");
    let p = plot(&mut s, "y = a x");
    let report = s.rename_refactor(a, "k").unwrap();
    assert_eq!(report.rewritten, 3);
    assert_eq!(report.nodes, vec![b, p]);
}

#[test]
fn move_before_sibling() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "1");
    let c = var(&mut s, "c", "1");
    s.move_before(c, a).unwrap();
    assert_eq!(s.roots(), &[c, a, b]);
    s.move_before(c, b).unwrap();
    assert_eq!(s.roots(), &[a, c, b]);
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    let d = s.add(Some(f), NodeContent::variable("d", "1")).unwrap();
    s.move_before(a, d).unwrap();
    assert_eq!(s.children(Some(f)), &[a, d]);
    s.move_before(a, a).unwrap();
}

// Decisions -------------------------------------------------------------------

#[test]
fn writes_are_separate_edges() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    let t = s.add(None, NodeContent::ticker("a -> a + 1", 100)).unwrap();
    assert_eq!(s.graph().writes(t), vec![a]);
    assert_eq!(s.graph().written_by(a), vec![t]);
    assert!(s.graph().edges().contains(&(t, a, EdgeKind::Writes)));
    assert!(s.graph().edges().contains(&(t, a, EdgeKind::Reads)));
    assert!(s.defines(t).is_empty(), "writes do not define");
}

#[test]
fn unknown_write_target_is_an_error() {
    let mut s = Scene::new();
    let t = s.add(None, NodeContent::action("zz -> 1")).unwrap();
    assert!(
        s.diagnostics(t)
            .iter()
            .any(|d| matches!(d.kind, DiagnosticKind::UnknownWriteTarget { .. }) && d.is_error())
    );
    let mut ev = |_: &Scene, _: NodeId| NodeResult::ok("");
    s.evaluate_dirty(&mut ev);
    assert!(s.status(t).is_error());
    let sc = s
        .add(None, NodeContent::script(ScriptLanguage::Viscas, "qq = 2"))
        .unwrap();
    assert!(s.diagnostics(sc).iter().any(|d| d.is_error()));
}

#[test]
fn rename_refactor_rewrites_write_targets() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    let t = s.add(None, NodeContent::ticker("a -> a + 1", 100)).unwrap();
    s.rename_refactor(a, "n").unwrap();
    assert_eq!(main_src(&s, t), "n -> n + 1");
    assert!(s.diagnostics(t).is_empty());
}

#[test]
fn table_columns_are_bound_in_table_scope() {
    let mut col2 = Column::new("y", &[]);
    col2.formula = Some("x^2 + k".into());
    let c = NodeContent::table("T", vec![Column::new("x", &["1", "2"]), col2]);
    assert_eq!(refs(&c), vec!["k".to_string()]);
}

#[test]
fn colon_equals_definitions() {
    let opts = LineOptions {
        definition_op: DefinitionOp::ColonEq,
    };
    assert_eq!(
        parse_definition_with("a := 3", opts).unwrap(),
        NodeContent::variable("a", "3")
    );
    assert_eq!(
        parse_definition_with("f(x) := x^2", opts).unwrap(),
        NodeContent::function("f", &["x"], "x^2")
    );
    assert_eq!(
        parse_definition_with("x := 2", opts).unwrap(),
        NodeContent::variable("x", "2")
    );
    assert_eq!(
        parse_definition_with("a = 3", opts).unwrap().kind.tag(),
        KindTag::Plot
    );
    assert_eq!(
        parse_definition_with("{x < 0: 1, 2} = y", opts)
            .unwrap()
            .kind
            .tag(),
        KindTag::Plot
    );
    let f = NodeContent::function("f", &["x"], "x^2");
    assert_eq!(
        definition_line_with(&f, opts).as_deref(),
        Some("f(x) := x^2")
    );
}
