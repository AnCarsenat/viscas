//! Evaluator boundary and status storage.

use std::collections::HashMap;

use super::*;

/// Toy evaluator: variables whose expression is a number or a sum of
/// numbers and already-evaluated variables.
#[derive(Default)]
struct Toy {
    values: HashMap<NodeId, f64>,
    calls: Vec<NodeId>,
}

impl NodeEvaluator for Toy {
    fn evaluate(&mut self, scene: &Scene, node: NodeId) -> NodeResult {
        self.calls.push(node);
        let n = scene.get(node).unwrap();
        let NodeKind::Variable { expr } = &n.kind else {
            return NodeResult {
                status: Status::Ok,
                display: None,
            };
        };
        let mut total = 0.0;
        for term in expr.split('+') {
            let term = term.trim();
            if let Ok(v) = term.parse::<f64>() {
                total += v;
            } else if let Some(id) = scene.graph().definers(term).first() {
                match self.values.get(id) {
                    Some(v) => total += v,
                    None => return NodeResult::error(format!("`{term}` has no value")),
                }
            } else {
                return NodeResult::error(format!("cannot evaluate `{term}`"));
            }
        }
        self.values.insert(node, total);
        NodeResult::ok(format!("{total}"))
    }
}

#[test]
fn evaluates_in_dependency_order() {
    let mut s = Scene::new();
    let c = var(&mut s, "c", "b + 1");
    let b = var(&mut s, "b", "a + 1");
    let a = var(&mut s, "a", "1");
    let mut ev = Toy::default();
    let order = s.evaluate_dirty(&mut ev);
    assert_eq!(order, vec![a, b, c]);
    assert_eq!(ev.calls, vec![a, b, c]);
    assert_eq!(s.result(c).unwrap().display.as_deref(), Some("3"));
    assert_eq!(*s.status(c), Status::Ok);
}

#[test]
fn only_dirty_nodes_are_reevaluated() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    let b = var(&mut s, "b", "a + 1");
    let other = var(&mut s, "o", "5");
    let mut ev = Toy::default();
    s.evaluate_dirty(&mut ev);
    ev.calls.clear();
    s.set_source(a, SourceField::Main, "10").unwrap();
    assert_eq!(*s.status(b), Status::Pending);
    assert_eq!(*s.status(other), Status::Ok);
    s.evaluate_dirty(&mut ev);
    assert_eq!(ev.calls, vec![a, b]);
    assert_eq!(s.result(b).unwrap().display.as_deref(), Some("11"));
}

#[test]
fn parse_errors_skip_the_evaluator() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1 +");
    let mut ev = Toy::default();
    s.evaluate_dirty(&mut ev);
    assert!(ev.calls.is_empty());
    match s.status(a) {
        Status::Error { field, span, .. } => {
            assert_eq!(*field, Some(SourceField::Main));
            assert!(span.is_some());
        }
        st => panic!("{st:?}"),
    }
}

#[test]
fn cycles_skip_the_evaluator() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "b");
    let b = var(&mut s, "b", "a");
    let mut ev = Toy::default();
    s.evaluate_dirty(&mut ev);
    assert!(ev.calls.is_empty());
    for id in [a, b] {
        match s.status(id) {
            Status::Error { message, .. } => assert!(message.contains("circular")),
            st => panic!("{st:?}"),
        }
    }
}

#[test]
fn errors_propagate_downstream() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1 +");
    let b = var(&mut s, "b", "a + 1");
    let c = var(&mut s, "c", "b + 1");
    let mut ev = Toy::default();
    s.evaluate_dirty(&mut ev);
    assert!(ev.calls.is_empty());
    assert!(s.status(a).is_error());
    match s.status(c) {
        Status::Error { message, .. } => assert!(message.contains("`b`"), "{message}"),
        st => panic!("{st:?}"),
    }
    // Fixing the input clears the whole chain.
    s.set_source(a, SourceField::Main, "1").unwrap();
    s.evaluate_dirty(&mut ev);
    assert_eq!(ev.calls, vec![a, b, c]);
    assert_eq!(*s.status(c), Status::Ok);
}

#[test]
fn unknown_names_are_left_to_the_evaluator() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "zz");
    let mut ev = Toy::default();
    s.evaluate_dirty(&mut ev);
    assert_eq!(ev.calls, vec![a]);
    assert!(s.status(a).is_error());
}

#[test]
fn closure_evaluator_and_status_events() {
    let mut s = Scene::new();
    let a = var(&mut s, "a", "1");
    s.drain_events();
    let mut ev = |_: &Scene, _: NodeId| NodeResult::ok("1");
    s.evaluate_dirty(&mut ev);
    assert!(s.drain_events().contains(&SceneEvent::Changed {
        id: a,
        what: Change::Status
    }));
    // Re-evaluation goes Pending -> Ok again.
    s.mark_dirty(a);
    s.drain_events();
    s.evaluate_dirty(&mut ev);
    let ev2 = s.drain_events();
    assert!(ev2.contains(&SceneEvent::Changed {
        id: a,
        what: Change::Status
    }));
}

#[test]
fn set_result_on_missing_node_is_ignored() {
    let mut s = Scene::new();
    s.set_result(NodeId(5), NodeResult::ok("x"));
    assert!(s.result(NodeId(5)).is_none());
    assert_eq!(*s.status(NodeId(5)), Status::Pending);
}

#[test]
fn status_helpers() {
    assert!(Status::error("x").is_error());
    assert!(!Status::Ok.is_error());
    assert_eq!(Status::default(), Status::Pending);
    assert_eq!(NodeResult::error("bad").status, Status::error("bad"));
}
