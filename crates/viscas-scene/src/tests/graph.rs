//! Dependency graph: resolution, diagnostics, traversal, dirty tracking.

use std::sync::Arc;

use super::*;

fn has_kind(s: &Scene, id: NodeId, f: impl Fn(&DiagnosticKind) -> bool) -> bool {
    s.diagnostics(id).iter().any(|d| f(&d.kind))
}

fn in_cycle_diag(s: &Scene, id: NodeId) -> bool {
    has_kind(s, id, |k| matches!(k, DiagnosticKind::Cycle { .. }))
}

#[test]
fn edges_follow_references() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a + 1");
    let f = func(&mut s, "f", &["x"], "x b");
    let p = plot(&mut s, "y = f(x)");
    assert_eq!(s.graph().direct_upstream(b), vec![a]);
    assert_eq!(s.graph().direct_upstream(p), vec![f]);
    assert_eq!(s.graph().direct_downstream(a), vec![b]);
    assert!(s.graph().edges().contains(&(f, b)));
}

#[test]
fn transitive_upstream_and_downstream() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a + 1");
    let c = var(&mut s, "c", "b * 2");
    let d = var(&mut s, "d", "a + c");
    let e = var(&mut s, "e", "7");
    assert_eq!(s.upstream(d), vec![a, b, c]);
    assert_eq!(s.downstream(a), vec![b, c, d]);
    assert!(s.downstream(e).is_empty());
    assert!(s.upstream(a).is_empty());
    assert_eq!(s.related(b), vec![a, c, d]);
}

#[test]
fn topological_order_puts_inputs_first() {
    let mut s = Scene::new();
    // Defined in reverse dependency order.
    let c = var(&mut s, "c", "b + 1");
    let b = var(&mut s, "b", "a + 1");
    let a = var(&mut s, "a", "1");
    let topo = s.topo_order().to_vec();
    let pos = |id| topo.iter().position(|&x| x == id).unwrap();
    assert!(pos(a) < pos(b));
    assert!(pos(b) < pos(c));
    assert_eq!(topo.len(), 3);
}

#[test]
fn diamond_dependencies() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a");
    let c = var(&mut s, "c", "a");
    let d = var(&mut s, "d", "b + c");
    assert_eq!(s.downstream(a), vec![b, c, d]);
    assert_eq!(s.upstream(d), vec![a, b, c]);
}

#[test]
fn self_reference_is_a_cycle() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "a + 1");
    assert!(in_cycle_diag(&s, a));
    assert_eq!(s.graph().cycles(), &[vec![a]]);
}

#[test]
fn two_node_cycle_reported_on_both() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "b + 1");
    let b = var(&mut s, "b", "a - 1");
    let c = var(&mut s, "c", "a");
    assert!(in_cycle_diag(&s, a));
    assert!(in_cycle_diag(&s, b));
    assert!(!in_cycle_diag(&s, c));
    assert!(s.graph().in_cycle(a));
    let msg = &s
        .diagnostics(a)
        .iter()
        .find(|d| matches!(d.kind, DiagnosticKind::Cycle { .. }))
        .unwrap()
        .message;
    assert!(msg.contains("circular"), "{msg}");
    assert!(msg.contains('a') && msg.contains('b'));
}

#[test]
fn long_cycle_lists_all_members() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "c");
    let b = var(&mut s, "b", "a");
    let c = var(&mut s, "c", "b");
    let d = var(&mut s, "d", "c");
    let members = match &s
        .diagnostics(b)
        .iter()
        .find(|d| matches!(d.kind, DiagnosticKind::Cycle { .. }))
        .unwrap()
        .kind
    {
        DiagnosticKind::Cycle { members } => members.clone(),
        _ => unreachable!(),
    };
    assert_eq!(members, vec![a, b, c]);
    assert!(!in_cycle_diag(&s, d));
    assert_eq!(s.topo_order().len(), 4);
}

#[test]
fn breaking_a_cycle_clears_diagnostics() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "b");
    let b = var(&mut s, "b", "a");
    s.set_source(b, SourceField::Main, "2").unwrap();
    assert!(!in_cycle_diag(&s, a));
    assert!(s.graph().cycles().is_empty());
}

#[test]
fn recursive_function_is_not_a_cycle() {
    let mut s = Scene::new();
    let f = func(&mut s, "f", &["n"], "{n <= 1: 1, n f(n - 1)}");
    assert!(s.diagnostics(f).is_empty());
    assert!(s.graph().direct_upstream(f).is_empty());
}

#[test]
fn mutual_function_recursion_is_allowed() {
    let mut s = Scene::new();
    let f = func(&mut s, "even", &["n"], "{n == 0: 1, odd(n - 1)}");
    let g = func(&mut s, "odd", &["n"], "{n == 0: 0, even(n - 1)}");
    assert!(!in_cycle_diag(&s, f));
    assert!(!in_cycle_diag(&s, g));
    assert_eq!(s.graph().direct_upstream(f), vec![g]);
}

#[test]
fn cycle_through_variable_and_function_is_an_error() {
    let mut s = Scene::new();
    let f = func(&mut s, "f", &["x"], "x + a");
    let a = var(&mut s, "a", "f(1)");
    assert!(in_cycle_diag(&s, f));
    assert!(in_cycle_diag(&s, a));
}

#[test]
fn unknown_names_are_reported_with_spans() {
    let mut s = Scene::new();
    let b = var(&mut s, "b", "1 + zz + zz");
    let unknown: Vec<&Diagnostic> = s
        .diagnostics(b)
        .iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::UnknownName { .. }))
        .collect();
    assert_eq!(unknown.len(), 1, "reported once per name");
    assert_eq!(unknown[0].span.unwrap().start, 4);
    assert_eq!(unknown[0].field, Some(SourceField::Main));
    assert!(!unknown[0].is_error());
}

#[test]
fn defining_a_missing_name_resolves_it() {
    let mut s = Scene::new();
    let b = var(&mut s, "b", "q + 1");
    assert!(!s.diagnostics(b).is_empty());
    let q = var(&mut s, "q", "2");
    assert!(s.diagnostics(b).is_empty());
    assert_eq!(s.graph().direct_upstream(b), vec![q]);
}

#[test]
fn builtins_are_not_unknown() {
    let mut s = Scene::new();
    let b = var(&mut s, "b", "sin(pi) + sqrt 2 + e");
    assert!(s.diagnostics(b).is_empty());
}

#[test]
fn custom_builtins() {
    let mut s = Scene::new();
    let b = var(&mut s, "b", "5 km");
    assert!(!s.diagnostics(b).is_empty());
    s.set_builtins(Arc::new(|n: &str| n == "km"));
    assert!(s.diagnostics(b).is_empty());
    assert!(s.is_dirty(b));
}

#[test]
fn duplicate_definitions_are_reported_on_all() {
    // Duplicates cannot be created through the API but can come from
    // documents or from script functions.
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let sc = s
        .add(
            None,
            NodeContent::script(ScriptLanguage::Viscas, "fn a(x) { return x }"),
        )
        .unwrap();
    let u = var(&mut s, "u", "a");
    for id in [a, sc] {
        assert!(has_kind(&s, id, |k| matches!(
            k,
            DiagnosticKind::DuplicateDefinition { .. }
        )));
    }
    assert!(has_kind(&s, u, |k| matches!(
        k,
        DiagnosticKind::AmbiguousReference { .. }
    )));
    assert_eq!(s.graph().direct_upstream(u), vec![a, sc]);
    assert_eq!(s.graph().definers("a"), &[a, sc]);
}

#[test]
fn split_names_resolve_letterwise() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "2");
    let b = var(&mut s, "b", "3");
    let c = var(&mut s, "c", "ab + 1");
    assert_eq!(s.graph().direct_upstream(c), vec![a, b]);
    assert!(s.diagnostics(c).is_empty());
    assert!(matches!(s.graph().resolutions(c)[0], Resolution::Split(_)));
}

#[test]
fn split_respects_bound_letters() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "2");
    let p = plot(&mut s, "y = ax");
    assert_eq!(s.graph().direct_upstream(p), vec![a]);
    assert!(s.diagnostics(p).is_empty());
}

#[test]
fn split_with_missing_letter_is_unknown() {
    let mut s = Scene::new();
    var(&mut s, "a", "2");
    let c = var(&mut s, "c", "aq");
    assert!(has_kind(
        &s,
        c,
        |k| matches!(k, DiagnosticKind::UnknownName { name } if name == "aq")
    ));
}

#[test]
fn whole_name_wins_over_split() {
    let mut s = Scene::new();
    var(&mut s, "a", "2");
    var(&mut s, "b", "3");
    let ab = var(&mut s, "ab", "4");
    let c = var(&mut s, "c", "ab");
    assert_eq!(s.graph().direct_upstream(c), vec![ab]);
}

#[test]
fn plot_variables_do_not_bind_to_nodes() {
    let mut s = Scene::new();
    let x = s.add(None, NodeContent::variable("x_0", "1")).unwrap();
    let p = plot(&mut s, "y = x + x_0");
    assert_eq!(s.graph().direct_upstream(p), vec![x]);
}

#[test]
fn parse_errors_become_diagnostics() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "(1 +");
    assert!(has_kind(&s, a, |k| matches!(k, DiagnosticKind::Parse)));
    assert!(s.diagnostics(a)[0].span.is_some());
}

#[test]
fn nodes_with_diagnostics_lists_offenders() {
    let mut s = Scene::new();
    var(&mut s, "a", "1");
    let b = var(&mut s, "b", "nope");
    assert_eq!(s.graph().nodes_with_diagnostics(), vec![b]);
}

#[test]
fn table_member_access_depends_on_table() {
    let mut s = Scene::new();
    let t = s
        .add(
            None,
            NodeContent::table("T", vec![Column::new("x_1", &["1"])]),
        )
        .unwrap();
    let m = var(&mut s, "m", "mean(T.x_1)");
    assert_eq!(s.graph().direct_upstream(m), vec![t]);
}

#[test]
fn regression_parameters_feed_downstream() {
    let mut s = Scene::new();
    let t = s
        .add(
            None,
            NodeContent::table(
                "T",
                vec![Column::new("x", &["1", "2"]), Column::new("y", &["2", "4"])],
            ),
        )
        .unwrap();
    let r = s
        .add(
            None,
            NodeContent::regression("T.y ~ m T.x + k", &["m", "k"]),
        )
        .unwrap();
    let p = plot(&mut s, "y = m x + k");
    assert_eq!(s.graph().direct_upstream(r), vec![t]);
    assert_eq!(s.graph().direct_upstream(p), vec![r]);
}

#[test]
fn action_reads_create_edges_but_writes_do_not() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "2");
    let act = s.add(None, NodeContent::action("a -> b + 1")).unwrap();
    assert_eq!(s.graph().direct_upstream(act), vec![b]);
    assert!(s.downstream(act).is_empty());
    assert!(s.downstream(a).is_empty());
    assert_eq!(s.analysis(act).unwrap().writes, vec!["a"]);
}

#[test]
fn ticker_self_update_is_not_a_cycle() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "0");
    let t = s.add(None, NodeContent::ticker("a -> a + 1", 100)).unwrap();
    assert!(s.diagnostics(t).is_empty());
    assert_eq!(s.graph().direct_upstream(t), vec![a]);
}

#[test]
fn removing_a_definer_makes_dependents_unknown() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a");
    s.remove(a).unwrap();
    assert!(has_kind(&s, b, |k| matches!(
        k,
        DiagnosticKind::UnknownName { .. }
    )));
    assert!(s.graph().direct_upstream(b).is_empty());
}

#[test]
fn renaming_rewires_edges() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "c");
    assert!(s.graph().direct_upstream(b).is_empty());
    s.rename(a, Some("c")).unwrap();
    assert_eq!(s.graph().direct_upstream(b), vec![a]);
}

#[test]
fn deep_chain_does_not_overflow() {
    // Built as a document (one graph build) to exercise the iterative
    // Tarjan on a 5000-deep chain without paying per-insert rebuilds.
    let n = 5000;
    let nodes = (0..n)
        .map(|i| DocNode {
            id: NodeId(i + 1),
            name: Some(format!("v_{i}")),
            kind: NodeKind::Variable {
                expr: if i == 0 {
                    "1".into()
                } else {
                    format!("v_{} + 1", i - 1)
                },
            },
            visible: true,
            locked: false,
            style: Style::default(),
            props: Default::default(),
            children: Vec::new(),
        })
        .collect();
    let s = Scene::from_document(Document {
        version: FORMAT_VERSION,
        next_id: 0,
        nodes,
        layout: None,
    })
    .unwrap();
    let first = s.lookup("v_0").unwrap();
    let last = s.lookup(&format!("v_{}", n - 1)).unwrap();
    assert_eq!(s.downstream(first).len(), n as usize - 1);
    assert_eq!(s.upstream(last).len(), n as usize - 1);
    assert_eq!(s.topo_order()[0], first);
    assert_eq!(*s.topo_order().last().unwrap(), last);
}

#[test]
fn hundreds_of_inserts_stay_consistent() {
    let mut s = Scene::new();
    var(&mut s, "v_0", "1");
    for i in 1..200 {
        var(&mut s, &format!("v_{i}"), &format!("v_{} + 1", i - 1));
    }
    let first = s.lookup("v_0").unwrap();
    assert_eq!(s.downstream(first).len(), 199);
}

// ---------------------------------------------------------------------------
// Dirty propagation
// ---------------------------------------------------------------------------

#[test]
fn new_nodes_are_dirty() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    assert!(s.is_dirty(a));
    assert_eq!(s.take_dirty(), vec![a]);
    assert!(!s.is_dirty(a));
    assert!(s.take_dirty().is_empty());
}

#[test]
fn editing_marks_downstream_dirty_in_topo_order() {
    let mut s = Scene::new();
    let c = var(&mut s, "c", "b * 2");
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a + 1");
    let other = var(&mut s, "o", "5");
    s.take_dirty();
    s.set_source(a, SourceField::Main, "2").unwrap();
    assert_eq!(s.take_dirty(), vec![a, b, c]);
    assert!(!s.is_dirty(other));
}

#[test]
fn style_changes_do_not_dirty() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.take_dirty();
    s.set_visible(a, false).unwrap();
    s.set_style(
        a,
        Style {
            opacity: 0.5,
            ..Style::default()
        },
    )
    .unwrap();
    s.set_prop(a, "k", 1i64).unwrap();
    assert!(s.take_dirty().is_empty());
}

#[test]
fn adding_definer_dirties_previously_unknown_dependents() {
    let mut s = Scene::new();
    let b = var(&mut s, "b", "q + 1");
    s.take_dirty();
    let q = var(&mut s, "q", "1");
    assert_eq!(s.take_dirty(), vec![q, b]);
}

#[test]
fn removing_definer_dirties_dependents() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a + 1");
    let c = var(&mut s, "c", "b");
    s.take_dirty();
    s.remove(a).unwrap();
    assert_eq!(s.take_dirty(), vec![b, c]);
}

#[test]
fn rename_dirties_old_and_new_dependents() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a");
    let c = var(&mut s, "c", "z");
    s.take_dirty();
    s.rename(a, Some("z")).unwrap();
    let dirty = s.take_dirty();
    assert!(dirty.contains(&a) && dirty.contains(&b) && dirty.contains(&c));
    let pos = |id| dirty.iter().position(|&x| x == id).unwrap();
    assert!(pos(a) < pos(c));
}

#[test]
fn move_does_not_dirty() {
    let mut s = Scene::new();
    let f = s.add(None, NodeContent::folder("F")).unwrap();
    let a = var(&mut s, "a", "1");
    s.take_dirty();
    s.move_node(a, Some(f), 0).unwrap();
    assert!(s.take_dirty().is_empty());
}

#[test]
fn mark_dirty_manually() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "random()");
    let b = var(&mut s, "b", "a");
    s.take_dirty();
    s.mark_dirty(a);
    assert_eq!(s.dirty(), vec![a, b]);
    s.mark_all_dirty();
    assert_eq!(s.take_dirty().len(), 2);
    s.mark_dirty(NodeId(999));
    assert!(s.take_dirty().is_empty());
}

#[test]
fn removed_nodes_leave_the_dirty_set() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.remove(a).unwrap();
    assert!(s.take_dirty().is_empty());
}
