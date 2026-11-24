//! Console evaluation, commands and history.

use viscas_core::eval::AngleUnit;
use viscas_core::syntax::DefinitionOp;
use viscas_scene::KindTag;

use super::*;

fn first(out: &ConsoleOutput) -> &ConsoleItem {
    assert!(out.error.is_none(), "{:?}", out.error);
    &out.results[0]
}

#[test]
fn evaluates_exact_and_approx() {
    let mut e = engine();
    let out = e.console_eval("1/3 + 1/6");
    let it = first(&out);
    assert_eq!(it.exact, "1/2");
    assert_eq!(it.approx, "0.5");
    assert_eq!(it.kind, ItemKind::Value);
    assert_eq!(out.input_echo, "1/3 + 1/6");
}

#[test]
fn console_sees_scene_nodes() {
    let mut e = engine();
    var(&mut e, "a", "2");
    func(&mut e, "f", &["x"], "a x + 1");
    assert_eq!(first(&e.console_eval("f(a)")).exact, "5");
}

#[test]
fn console_sees_node_updates() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    assert_eq!(first(&e.console_eval("a")).exact, "2");
    set(&mut e, a, "7");
    assert_eq!(first(&e.console_eval("a")).exact, "7");
}

#[test]
fn console_definitions_stay_in_the_console() {
    let mut e = engine();
    let out = e.console_eval("b := 5");
    assert_eq!(first(&out).kind, ItemKind::Definition { name: "b".into() });
    assert!(e.scene().is_empty());
    assert_eq!(first(&e.console_eval("b^2")).exact, "25");
    assert_eq!(e.console_names(), vec!["b".to_string()]);
}

#[test]
fn console_definition_of_node_name_updates_the_node() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    let b = var(&mut e, "b", "a + 1");
    let out = e.console_eval("a := 10");
    assert_eq!(
        first(&out).kind,
        ItemKind::UpdatedNode {
            node: a,
            name: "a".into()
        }
    );
    assert_eq!(first(&out).exact, "10");
    assert_eq!(exact(&e, b), "11");
    assert!(e.console_names().is_empty(), "never shadows a node");
    assert!(e.undo());
    assert_eq!(exact(&e, a), "2");
    assert_eq!(first(&e.console_eval("a")).exact, "2");
}

#[test]
fn console_function_redefines_node_function() {
    let mut e = engine();
    let f = func(&mut e, "f", &["x"], "x");
    e.console_eval("f(x) := 3x");
    let v = var(&mut e, "v", "f(2)");
    assert_eq!(exact(&e, v), "6");
    assert_eq!(e.scene().get(f).unwrap().kind.tag(), KindTag::Function);
}

#[test]
fn relation_has_no_value() {
    let mut e = engine();
    let out = e.console_eval("y = x^2");
    assert_eq!(
        first(&out).kind,
        ItemKind::Relation {
            kind: "explicit y".into()
        }
    );
}

#[test]
fn parse_error_has_span() {
    let mut e = engine();
    let out = e.console_eval("1 + ");
    let err = out.error.unwrap();
    assert!(err.span.is_some());
    assert!(out.results.is_empty());
}

#[test]
fn unknown_name_error_suggests() {
    let mut e = engine();
    let out = e.console_eval("sni(1)");
    let err = out.error.as_ref().unwrap();
    assert!(err.message.contains("sni"), "{}", err.message);
    let span = err.span.unwrap();
    assert_eq!(&"sni(1)"[span.start..span.end], "sni");
    assert!(
        out.suggestions.contains(&"sin".to_string()),
        "{:?}",
        out.suggestions
    );
}

#[test]
fn suggestions_include_nodes() {
    let mut e = engine();
    var(&mut e, "alpha", "1");
    let out = e.console_eval("alpah + 1");
    assert!(
        out.suggestions.contains(&"alpha".to_string()),
        "{:?}",
        out.suggestions
    );
}

#[test]
fn ans_and_ans2() {
    let mut e = engine();
    e.console_eval("6");
    e.console_eval("7");
    assert_eq!(first(&e.console_eval("ans * ans2")).exact, "42");
}

#[test]
fn let_locals_persist() {
    let mut e = engine();
    e.console_eval("let k = 3");
    assert_eq!(first(&e.console_eval("k + 1")).exact, "4");
}

#[test]
fn multiple_statements() {
    let mut e = engine();
    let out = e.console_eval("q := 2; q + 1");
    assert_eq!(out.results.len(), 2);
    assert_eq!(out.results[1].exact, "3");
}

#[test]
fn pin_definition_creates_node_and_undo_removes_it() {
    let mut e = engine();
    e.console_eval("a := 3");
    let out = e.console_eval(":pin");
    let ItemKind::Pinned { node } = first(&out).kind.clone() else {
        panic!("{out:?}");
    };
    assert_eq!(e.scene().get(node).unwrap().name.as_deref(), Some("a"));
    assert_eq!(exact(&e, node), "3");
    assert!(e.console_names().is_empty(), "the node provides `a` now");
    assert_eq!(first(&e.console_eval("a + 1")).exact, "4");
    assert!(e.undo());
    assert!(!e.scene().contains(node));
    // The console definition is back, consistently with the undo.
    assert_eq!(first(&e.console_eval("a")).exact, "3");
    assert_eq!(e.console_names(), vec!["a".to_string()]);
}

#[test]
fn pinned_node_is_reactive() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    e.console_eval("b := a + 1");
    let out = e.console_eval(":pin");
    let ItemKind::Pinned { node } = first(&out).kind.clone() else {
        panic!()
    };
    set(&mut e, a, "10");
    assert_eq!(exact(&e, node), "11");
}

#[test]
fn pin_value_and_relation() {
    let mut e = engine();
    e.console_eval("2 + 3");
    let out = e.console_eval(":pin");
    let ItemKind::Pinned { node } = first(&out).kind.clone() else {
        panic!()
    };
    assert_eq!(exact(&e, node), "5");
    e.console_eval("y = x^2");
    let out = e.console_eval(":pin");
    let ItemKind::Pinned { node } = first(&out).kind.clone() else {
        panic!()
    };
    let n = e.scene().get(node).unwrap();
    assert_eq!(n.kind.tag(), KindTag::Plot);
    assert!(e.node_color(node).is_some());
    assert!(e.plot_of(node).is_some());
}

#[test]
fn pin_function() {
    let mut e = engine();
    e.console_eval("g(x) := x^3");
    let out = e.console_eval(":pin");
    let ItemKind::Pinned { node } = first(&out).kind.clone() else {
        panic!()
    };
    assert_eq!(e.scene().get(node).unwrap().kind.tag(), KindTag::Function);
}

#[test]
fn pin_without_result_errors() {
    let mut e = engine();
    assert!(e.console_eval(":pin").error.is_some());
    e.console_eval("1/0");
    assert!(
        e.console_eval(":pin").error.is_some(),
        "errors are not pinnable"
    );
}

#[test]
fn pin_twice_errors() {
    let mut e = engine();
    e.console_eval("a := 3");
    e.console_eval(":pin");
    assert!(e.console_eval(":pin").error.is_some());
}

#[test]
fn pin_existing_name_errors() {
    let mut e = engine();
    e.console_eval("a := 3");
    var(&mut e, "a", "1");
    let out = e.console_eval(":pin");
    assert!(out.error.unwrap().message.contains("already exists"));
}

#[test]
fn mode_switches() {
    let mut e = engine();
    let a = var(&mut e, "a", "sin(30)");
    let out = e.console_eval(":mode deg");
    assert!(first(&out).exact.contains("deg"));
    assert_eq!(e.settings().angle, AngleUnit::Degrees);
    assert_eq!(exact(&e, a), "1/2");
    assert_eq!(first(&e.console_eval("cos(60)")).exact, "1/2");
    e.console_eval(":mode rad approx complex");
    assert_eq!(e.settings().angle, AngleUnit::Radians);
    assert!(e.settings().approx);
    assert!(e.settings().complex);
    assert_eq!(first(&e.console_eval("sqrt(-1)")).exact, "i");
    e.console_eval(":mode exact real");
    assert!(!e.settings().approx && !e.settings().complex);
    assert!(e.console_eval("sqrt(-1)").error.is_some());
}

#[test]
fn mode_without_args_reports() {
    let mut e = engine();
    assert_eq!(
        first(&e.console_eval(":mode")).exact,
        "mode: exact · rad · real"
    );
}

#[test]
fn bad_mode_errors_with_suggestion() {
    let mut e = engine();
    let out = e.console_eval(":mode aprox");
    assert!(out.error.is_some());
    assert_eq!(out.suggestions, vec!["approx".to_string()]);
    assert!(!e.settings().approx);
}

#[test]
fn help_topics() {
    let mut e = engine();
    assert!(first(&e.console_eval(":help sin")).exact.contains("sine"));
    assert!(
        first(&e.console_eval(":help arcsin"))
            .exact
            .contains("alias")
    );
    assert!(first(&e.console_eval(":help pi")).exact.contains("3.14"));
    assert!(first(&e.console_eval(":help :pin")).exact.contains("node"));
    assert!(first(&e.console_eval(":help mode")).exact.contains("deg"));
    assert!(e.console_eval(":help").results.len() >= 5);
}

#[test]
fn help_on_nodes_and_console_names() {
    let mut e = engine();
    var(&mut e, "a", "1/2");
    let t = first(&e.console_eval(":help a")).exact.clone();
    assert!(t.contains("a := 1/2") && t.ends_with("= 1/2"), "{t}");
    e.console_eval("z := 9");
    assert!(first(&e.console_eval(":help z")).exact.contains("console"));
    let out = e.console_eval(":help sinn");
    assert!(out.error.is_some());
    assert!(out.suggestions.contains(&"sin".to_string()));
}

#[test]
fn clear_resets_session() {
    let mut e = engine();
    e.console_eval("b := 5");
    let out = e.console_eval(":clear");
    assert!(out.cleared);
    assert!(e.console_names().is_empty());
    assert!(e.console_eval("b").error.is_some());
    assert!(!e.history().is_empty(), "input history survives :clear");
}

#[test]
fn vars_lists_scene_and_console() {
    let mut e = engine();
    var(&mut e, "a", "2");
    e.console_eval("c := 3");
    let out = e.console_eval(":vars");
    let kinds: Vec<&ItemKind> = out.results.iter().map(|r| &r.kind).collect();
    assert!(kinds.contains(&&ItemKind::Variable {
        name: "a".into(),
        scene: true
    }));
    assert!(kinds.contains(&&ItemKind::Variable {
        name: "c".into(),
        scene: false
    }));
}

#[test]
fn vars_when_empty() {
    let mut e = engine();
    assert_eq!(first(&e.console_eval(":vars")).kind, ItemKind::Info);
}

#[test]
fn unknown_command_suggests() {
    let mut e = engine();
    let out = e.console_eval(":hlep");
    assert!(out.error.is_some());
    assert_eq!(out.suggestions, vec![":help".to_string()]);
}

#[test]
fn history_records_and_dedupes() {
    let mut e = engine();
    e.console_eval("1");
    e.console_eval("1");
    e.console_eval("  2 ");
    e.console_eval("");
    assert_eq!(e.history(), &["1".to_string(), "2".to_string()]);
}

#[test]
fn history_is_persistable_and_bounded() {
    let mut e = engine();
    let many: Vec<String> = (0..HISTORY_LIMIT + 10).map(|i| i.to_string()).collect();
    e.set_history(many);
    assert_eq!(e.history().len(), HISTORY_LIMIT);
    assert_eq!(e.history()[0], "10");
    e.console_eval("x_1 := 1");
    assert_eq!(e.history().len(), HISTORY_LIMIT);
    assert_eq!(e.history().last().unwrap(), "x_1 := 1");
}

#[test]
fn eq_definition_operator_in_console() {
    let mut e = engine();
    e.set_settings(EngineSettings {
        definition_op: DefinitionOp::Eq,
        ..*e.settings()
    });
    assert_eq!(
        first(&e.console_eval("a = 3")).kind,
        ItemKind::Definition { name: "a".into() }
    );
    assert!(matches!(
        first(&e.console_eval("y = x")).kind,
        ItemKind::Relation { .. }
    ));
}

#[test]
fn item_display_follows_mode() {
    let mut e = engine();
    let out = e.console_eval("1/4");
    let it = first(&out);
    assert_eq!(it.display(false), "1/4");
    assert_eq!(it.display(true), "0.25");
}

#[test]
fn console_does_not_touch_undo_history() {
    let mut e = engine();
    e.console_eval("b := 5");
    e.console_eval("2 + 2");
    assert!(!e.scene().can_undo());
}

// ----------------------------------------------------------------------
// Review regressions
// ----------------------------------------------------------------------

#[test]
fn pin_also_pins_console_dependencies_in_one_undo_step() {
    let mut e = engine();
    e.console_eval("b := 2");
    e.console_eval("g(x) := b x");
    e.console_eval("c := g(3) + 1");
    let out = e.console_eval(":pin");
    assert!(out.error.is_none(), "{:?}", out.error);
    let pinned: Vec<NodeId> = out
        .results
        .iter()
        .filter_map(|r| match r.kind {
            ItemKind::Pinned { node } => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(pinned.len(), 3, "b, g and c");
    let c = *pinned.last().unwrap();
    assert_eq!(exact(&e, c), "7");
    for name in ["b", "g", "c"] {
        assert!(e.scene().lookup(name).is_some(), "{name}");
    }
    assert!(e.undo());
    assert!(e.scene().is_empty(), "one undo removes the whole pin");
    assert_eq!(first(&e.console_eval("c")).exact, "7");
}

#[test]
fn pin_refuses_console_values_without_definition() {
    let mut e = engine();
    e.console_eval("5");
    e.console_eval("ans + 1");
    let out = e.console_eval(":pin");
    assert!(out.error.unwrap().message.contains("ans"));
    assert!(e.scene().is_empty());
}

#[test]
fn spans_are_offset_per_statement() {
    let mut e = engine();
    let input = "q1 := 1; 2 + qq";
    let out = e.console_eval(input);
    let span = out.error.unwrap().span.unwrap();
    assert_eq!(&input[span.start..span.end], "qq");
    assert_eq!(out.results.len(), 1, "the first statement ran");
}

#[test]
fn spans_respect_char_boundaries() {
    let mut e = engine();
    let input = "√(4) + π + qq";
    let out = e.console_eval(input);
    let span = out.error.unwrap().span.unwrap();
    assert!(input.is_char_boundary(span.start) && input.is_char_boundary(span.end));
    assert_eq!(&input[span.start..span.end], "qq");
}

#[test]
fn spans_from_called_function_bodies_are_dropped() {
    let mut e = engine();
    e.console_eval("h(x) := 1/(x - 2)");
    let out = e.console_eval("h(2) + 100000000");
    let err = out.error.unwrap();
    assert!(err.message.contains("zero"), "{}", err.message);
    assert_eq!(err.span, None);
}

#[test]
fn eq_rule_reads_plot_variables_as_relations() {
    let mut e = engine();
    e.set_settings(EngineSettings {
        definition_op: DefinitionOp::Eq,
        ..*e.settings()
    });
    assert!(matches!(
        first(&e.console_eval("a = x^2")).kind,
        ItemKind::Relation { .. }
    ));
    assert!(e.console_names().is_empty());
    assert_eq!(
        first(&e.console_eval("k = 3")).kind,
        ItemKind::Definition { name: "k".into() }
    );
    assert_eq!(
        first(&e.console_eval("sq(x) = x^2")).kind,
        ItemKind::Definition { name: "sq".into() }
    );
    assert!(matches!(
        first(&e.console_eval("t = 3")).kind,
        ItemKind::Relation { .. }
    ));
}

#[test]
fn pin_plan_pins_an_older_result_later_without_reevaluating() {
    let mut e = engine();
    e.console_eval("k := 2");
    e.console_eval("m := k + 1");
    let plan = e.pin_plan().unwrap();
    assert_eq!(plan.len(), 2, "dependency first, then the input");
    // Later inputs change the console state; the plan is unaffected.
    e.console_eval("k := 10");
    e.console_eval("7 * 6");
    let history = e.history().to_vec();
    let ids = e.apply_pin_plan(plan).unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(e.history(), &history[..], "nothing evaluated again");
    let m = e.scene().lookup("m").unwrap();
    assert_eq!(e.value(m).unwrap().exact, "3");
    // One undo step removes both nodes.
    assert!(e.undo());
    assert!(e.scene().lookup("m").is_none());
    assert!(e.scene().lookup("k").is_none());
}

#[test]
fn stale_pin_plan_skips_dependencies_now_in_the_scene() {
    let mut e = engine();
    e.console_eval("k := 2");
    e.console_eval("m := k + 1");
    let plan = e.pin_plan().unwrap();
    e.add_line(None, "k := 5").unwrap();
    let ids = e.apply_pin_plan(plan.clone()).unwrap();
    assert_eq!(ids.len(), 1);
    let m = e.scene().lookup("m").unwrap();
    assert_eq!(e.value(m).unwrap().exact, "6");
    // Pinning again: the input's own name is taken now.
    assert!(e.apply_pin_plan(plan).is_err());
}
