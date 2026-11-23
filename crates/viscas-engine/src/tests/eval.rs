//! Scene evaluation: references, updates, errors, settings.

use viscas_core::eval::AngleUnit;
use viscas_core::syntax::DefinitionOp;
use viscas_scene::{KindTag, NodeContent, ScriptLanguage, SourceField, Status};

use super::*;

#[test]
fn variable_value() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    assert_eq!(exact(&e, a), "2");
    assert_eq!(e.scene().status(a), &Status::Ok);
}

#[test]
fn cross_node_reference() {
    let mut e = engine();
    var(&mut e, "a", "2");
    let b = var(&mut e, "b", "a / 3");
    assert_eq!(exact(&e, b), "2/3");
}

#[test]
fn dependents_update_when_input_changes() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    let b = var(&mut e, "b", "a^2");
    set(&mut e, a, "5");
    assert_eq!(exact(&e, b), "25");
}

#[test]
fn reference_declared_before_definition() {
    let mut e = engine();
    let b = var(&mut e, "b", "a + 1");
    assert!(e.scene().status(b).is_error());
    var(&mut e, "a", "1");
    assert_eq!(exact(&e, b), "2");
}

#[test]
fn function_sees_variable_and_updates() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    func(&mut e, "f", &["x"], "a x");
    let c = var(&mut e, "c", "f(3)");
    assert_eq!(exact(&e, c), "6");
    set(&mut e, a, "3");
    assert_eq!(exact(&e, c), "9");
}

#[test]
fn plot_of_function_reevaluates_when_variable_changes() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    func(&mut e, "f", &["x"], "a x");
    let p = plot(&mut e, "y = f(x)");
    let rev = e.evaluator().revision(p);
    let eval_at = |e: &Engine| match &e.plot_of(p).unwrap().shape {
        PlotShape::ExplicitY(f) => f(1.5),
        other => panic!("{other:?}"),
    };
    assert_eq!(eval_at(&e), 3.0);
    set(&mut e, a, "4");
    assert_eq!(eval_at(&e), 6.0);
    assert!(e.evaluator().revision(p) > rev);
}

#[test]
fn unrelated_nodes_are_not_reevaluated() {
    let mut e = engine();
    let a = var(&mut e, "a", "2");
    let other = var(&mut e, "other", "7");
    let rev = e.evaluator().revision(other);
    set(&mut e, a, "3");
    assert_eq!(e.evaluator().revision(other), rev);
}

#[test]
fn evaluation_error_has_span_in_field() {
    let mut e = engine();
    let b = var(&mut e, "b", "1 + 1/0");
    let (msg, field, span) = error(&e, b);
    assert!(msg.contains("zero"), "{msg}");
    assert_eq!(field, Some(SourceField::Main));
    let span = span.expect("span");
    assert!(span.end <= "1 + 1/0".len() && span.start >= 4, "{span:?}");
}

#[test]
fn unknown_name_error_span() {
    let mut e = engine();
    let b = var(&mut e, "b", "2 + qq");
    let (msg, _, span) = error(&e, b);
    assert!(msg.contains("qq"), "{msg}");
    let span = span.unwrap();
    assert_eq!(&"2 + qq"[span.start..span.end], "qq");
}

#[test]
fn parse_error_reported_with_span() {
    let mut e = engine();
    let b = var(&mut e, "b", "2 +");
    let (_, field, span) = error(&e, b);
    assert_eq!(field, Some(SourceField::Main));
    assert!(span.is_some());
}

#[test]
fn errors_propagate_to_dependents() {
    let mut e = engine();
    let a = var(&mut e, "a", "1/0");
    let b = var(&mut e, "b", "a + 1");
    assert!(e.scene().status(a).is_error());
    let (msg, _, _) = error(&e, b);
    assert!(msg.contains('a'), "{msg}");
    assert!(e.value(b).is_none());
}

#[test]
fn fixing_an_error_recovers_dependents() {
    let mut e = engine();
    let a = var(&mut e, "a", "1/0");
    let b = var(&mut e, "b", "a + 1");
    set(&mut e, a, "1");
    assert_eq!(exact(&e, b), "2");
}

#[test]
fn erroring_input_does_not_leak_stale_value() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let b = var(&mut e, "b", "a + 1");
    set(&mut e, a, "1/0");
    assert!(e.value(a).is_none());
    assert!(e.scene().status(b).is_error());
    assert!(e.evaluator().lookup("a").is_none());
}

#[test]
fn cycles_are_reported() {
    let mut e = engine();
    let a = var(&mut e, "a", "b + 1");
    let b = var(&mut e, "b", "a + 1");
    let (msg_a, _, _) = error(&e, a);
    let (msg_b, _, _) = error(&e, b);
    assert!(msg_a.contains("circular"), "{msg_a}");
    assert!(msg_b.contains("circular"), "{msg_b}");
}

#[test]
fn recursive_function_is_allowed() {
    let mut e = engine();
    func(&mut e, "fac", &["n"], "{n <= 1: 1, n fac(n - 1)}");
    let v = var(&mut e, "v", "fac(5)");
    assert_eq!(exact(&e, v), "120");
}

#[test]
fn duplicate_definitions_are_errors() {
    let mut e = engine();
    let a1 = var(&mut e, "a", "1");
    let a2 = e
        .scene_mut()
        .add(None, NodeContent::variable("a", "2"))
        .map(Some)
        .unwrap_or(None);
    e.update();
    if let Some(a2) = a2 {
        assert!(e.scene().status(a1).is_error() || e.scene().status(a2).is_error());
    }
}

#[test]
fn removing_a_node_removes_its_definition() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let b = var(&mut e, "b", "a + 1");
    e.scene_mut().remove(a).unwrap();
    e.update();
    assert!(e.evaluator().lookup("a").is_none());
    assert!(e.scene().status(b).is_error());
}

#[test]
fn undo_and_redo_reevaluate() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let b = var(&mut e, "b", "a + 1");
    set(&mut e, a, "10");
    assert_eq!(exact(&e, b), "11");
    assert!(e.undo());
    assert_eq!(exact(&e, b), "2");
    assert!(e.redo());
    assert_eq!(exact(&e, b), "11");
}

#[test]
fn rename_refactor_keeps_values() {
    let mut e = engine();
    let a = var(&mut e, "a", "4");
    let b = var(&mut e, "b", "a / 2");
    e.scene_mut().rename_refactor(a, "c").unwrap();
    e.update();
    assert_eq!(exact(&e, b), "2");
    assert!(e.evaluator().lookup("a").is_none());
    assert!(e.evaluator().lookup("c").is_some());
}

#[test]
fn scene_result_display_is_set() {
    let mut e = engine();
    let a = var(&mut e, "a", "2/3");
    assert_eq!(e.scene().result(a).unwrap().display.as_deref(), Some("2/3"));
}

#[test]
fn exact_and_approx_renderings() {
    let mut e = engine();
    let a = var(&mut e, "a", "1/3");
    let v = e.value(a).unwrap();
    assert_eq!(v.exact, "1/3");
    assert_eq!(v.approx, "0.333333333333");
    assert_eq!(v.display(true), "0.333333333333");
}

#[test]
fn irrational_values_are_marked_inexact() {
    let mut e = engine();
    let a = var(&mut e, "a", "sqrt(2)");
    assert!(exact(&e, a).starts_with('≈'));
}

#[test]
fn approx_setting_changes_display() {
    let mut e = engine();
    let a = var(&mut e, "a", "1/4");
    e.set_settings(EngineSettings {
        approx: true,
        ..*e.settings()
    });
    assert_eq!(
        e.scene().result(a).unwrap().display.as_deref(),
        Some("0.25")
    );
    assert_eq!(exact(&e, a), "1/4");
}

#[test]
fn degree_mode_reevaluates() {
    let mut e = engine();
    let a = var(&mut e, "a", "sin(30)");
    assert!(exact(&e, a).starts_with('≈'));
    e.set_settings(EngineSettings {
        angle: AngleUnit::Degrees,
        ..*e.settings()
    });
    assert_eq!(exact(&e, a), "1/2");
}

#[test]
fn complex_mode_toggle() {
    let mut e = engine();
    let a = var(&mut e, "a", "sqrt(-4)");
    assert!(e.scene().status(a).is_error());
    e.set_settings(EngineSettings {
        complex: true,
        ..*e.settings()
    });
    assert_eq!(exact(&e, a), "2i");
}

#[test]
fn settings_change_bumps_every_revision() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let p = plot(&mut e, "y = x");
    let (ra, rp) = (e.evaluator().revision(a), e.evaluator().revision(p));
    e.set_settings(EngineSettings {
        precision_digits: 30,
        ..*e.settings()
    });
    assert!(e.evaluator().revision(a) > ra);
    assert!(e.evaluator().revision(p) > rp);
}

#[test]
fn unchanged_settings_do_nothing() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let r = e.evaluator().revision(a);
    e.set_settings(*e.settings());
    assert_eq!(e.evaluator().revision(a), r);
}

#[test]
fn slider_value_and_bounds() {
    let mut e = engine();
    let s = e
        .add_node(None, NodeContent::slider("s", "3", "0", "10"))
        .unwrap();
    assert_eq!(exact(&e, s), "3");
    let b = var(&mut e, "b", "2 s");
    assert_eq!(exact(&e, b), "6");
}

#[test]
fn slider_bound_error_maps_to_field() {
    let mut e = engine();
    let s = e
        .add_node(None, NodeContent::slider("s", "3", "1/0", "10"))
        .unwrap();
    let (_, field, _) = error(&e, s);
    assert_eq!(field, Some(SourceField::Min));
}

#[test]
fn slider_bound_must_be_real() {
    let mut e = engine();
    let s = e
        .add_node(None, NodeContent::slider("s", "3", "0", "[1, 2]"))
        .unwrap();
    let (_, field, _) = error(&e, s);
    assert_eq!(field, Some(SourceField::Max));
}

#[test]
fn point_node_value_and_plot() {
    let mut e = engine();
    let p = e.add_node(None, NodeContent::point("P", "(1, 2)")).unwrap();
    assert_eq!(exact(&e, p), "(1, 2)");
    assert_eq!(e.plot_of(p).unwrap().kind, PlotKind::Points);
    let q = var(&mut e, "q", "P.x + P.y");
    assert_eq!(exact(&e, q), "3");
}

#[test]
fn point_node_needs_a_point() {
    let mut e = engine();
    let p = e.add_node(None, NodeContent::point("P", "5")).unwrap();
    let (msg, _, _) = error(&e, p);
    assert!(msg.contains("point"), "{msg}");
}

#[test]
fn list_of_points_is_plotted_list_of_numbers_is_not() {
    let mut e = engine();
    let l = e
        .add_node(None, NodeContent::list("L", "[(1, 2), (3, 4)]"))
        .unwrap();
    assert!(e.plot_of(l).is_some());
    let m = e.add_node(None, NodeContent::list("M", "[1..5]")).unwrap();
    assert_eq!(exact(&e, m), "[1, 2, 3, 4, 5]");
    assert!(e.plot_of(m).is_none());
}

#[test]
fn unnamed_expression_does_not_define() {
    let mut e = engine();
    let x = e.add_node(None, NodeContent::expression("2 + 3")).unwrap();
    assert_eq!(exact(&e, x), "5");
    assert_eq!(e.evaluator().lookup("2 + 3"), None);
    assert!(!e.evaluator().defines("2 + 3"));
}

#[test]
fn function_node_value_is_a_function() {
    let mut e = engine();
    let f = func(&mut e, "f", &["x"], "x^2");
    assert!(exact(&e, f).starts_with("f(x)"));
}

#[test]
fn script_defines_functions() {
    let mut e = engine();
    let s = e
        .add_node(
            None,
            NodeContent::script(ScriptLanguage::Viscas, "fn sq(x) { x^2 }"),
        )
        .unwrap();
    assert!(!e.scene().status(s).is_error(), "{:?}", e.scene().status(s));
    let v = var(&mut e, "v", "sq(3)");
    assert_eq!(exact(&e, v), "9");
}

#[test]
fn rhai_scripts_report_unsupported() {
    let mut e = engine();
    let s = e
        .add_node(
            None,
            NodeContent::script(ScriptLanguage::Rhai, "let x = 1;"),
        )
        .unwrap();
    let (msg, _, _) = error(&e, s);
    assert!(msg.contains("Rhai"));
}

#[test]
fn builtins_are_known_to_the_scene() {
    let mut e = engine();
    let b = var(&mut e, "b", "sin(1) + pi");
    assert!(e.scene().diagnostics(b).is_empty());
}

#[test]
fn add_line_classifies() {
    let mut e = engine();
    let f = e.add_line(None, "g(x) := x + 1").unwrap();
    assert_eq!(e.scene().get(f).unwrap().kind.tag(), KindTag::Function);
    let p = e.add_line(None, "y = g(x)").unwrap();
    assert_eq!(e.scene().get(p).unwrap().kind.tag(), KindTag::Plot);
    assert!(e.add_line(None, "1 +").is_err());
}

#[test]
fn add_line_with_eq_definitions() {
    let mut e = engine();
    e.set_settings(EngineSettings {
        definition_op: DefinitionOp::Eq,
        ..*e.settings()
    });
    let a = e.add_line(None, "a = 3").unwrap();
    assert_eq!(e.scene().get(a).unwrap().kind.tag(), KindTag::Variable);
    assert_eq!(exact(&e, a), "3");
}

#[test]
fn plot_kinds_are_classified() {
    let cases = [
        ("y = x^2", PlotKind::ExplicitY),
        ("x^2", PlotKind::ExplicitY),
        ("x = y^2", PlotKind::ExplicitX),
        ("r = 1 + cos θ", PlotKind::Polar),
        ("θ = pi/4", PlotKind::PolarAngle),
        ("x^2 + y^2 = 4", PlotKind::Implicit),
        ("y < x", PlotKind::Inequality),
        ("r < 2", PlotKind::PolarInequality),
        ("(cos t, sin t)", PlotKind::Parametric),
        ("(1, 2)", PlotKind::Points),
        ("[(1, 2), (2, 3)]", PlotKind::Points),
    ];
    let mut e = engine();
    for (rel, kind) in cases {
        let p = plot(&mut e, rel);
        let got = e.plot_of(p).map(|c| c.kind);
        assert_eq!(got, Some(kind), "{rel}: {:?}", e.scene().status(p));
    }
}

#[test]
fn plot_with_unknown_name_errors() {
    let mut e = engine();
    let p = plot(&mut e, "y = qq x");
    let (msg, field, span) = error(&e, p);
    assert!(msg.contains("qq"), "{msg}");
    assert_eq!(field, Some(SourceField::Main));
    assert!(span.is_some());
    assert!(e.plot_of(p).is_none());
}

#[test]
fn equation_without_plot_variables_has_no_plot() {
    let mut e = engine();
    var(&mut e, "a", "1");
    let p = plot(&mut e, "a = 3");
    assert!(!e.scene().status(p).is_error());
    assert!(e.plot_of(p).is_none());
}

#[test]
fn parametric_needs_two_coordinates() {
    let mut e = engine();
    let p = plot(&mut e, "(t, t, t)");
    assert!(e.scene().status(p).is_error());
}

#[test]
fn interrupt_flag_is_shared() {
    let e = engine();
    let f = e.interrupt_flag();
    f.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        e.interrupt_flag()
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}

// ----------------------------------------------------------------------
// Review regressions
// ----------------------------------------------------------------------

#[test]
fn error_spans_from_other_bodies_are_dropped() {
    let mut e = engine();
    func(&mut e, "h", &["x"], "1/(x - 2)");
    let v = var(&mut e, "v", "h(2) + 1000000000");
    let (msg, _, span) = error(&e, v);
    assert!(msg.contains("zero"), "{msg}");
    assert_eq!(span, None);
}

#[test]
fn snapshot_is_reused_until_definitions_change() {
    let mut e = engine();
    let a = var(&mut e, "a", "1");
    let s1 = e.defs_snapshot();
    let s2 = e.defs_snapshot();
    assert!(std::sync::Arc::ptr_eq(&s1, &s2));
    set(&mut e, a, "2");
    let s3 = e.defs_snapshot();
    assert!(!std::sync::Arc::ptr_eq(&s1, &s3));
    assert_eq!(s3.get("a").map(|v| v.to_string()), Some("2".into()));
}

#[test]
fn plots_compile_against_the_finished_pass() {
    let mut e = engine();
    // The plot is created before its input: both evaluate in one pass.
    let p = e
        .scene_mut()
        .add(None, NodeContent::plot("y = m x"))
        .unwrap();
    e.scene_mut()
        .add(None, NodeContent::variable("m", "3"))
        .unwrap();
    e.update();
    match &e.plot_of(p).unwrap().shape {
        PlotShape::ExplicitY(f) => assert_eq!(f(2.0), 6.0),
        other => panic!("{other:?}"),
    }
}

#[test]
fn eq_rule_for_add_line_and_plot_nodes() {
    let mut e = engine();
    e.set_settings(EngineSettings {
        definition_op: DefinitionOp::Eq,
        ..*e.settings()
    });
    var(&mut e, "a", "4");
    let rel = e.add_line(None, "a = x^2").unwrap();
    assert_eq!(e.scene().get(rel).unwrap().kind.tag(), KindTag::Plot);
    assert!(e.plot_of(rel).is_some());
    let f = e.add_line(None, "sq(x) = x^2").unwrap();
    assert_eq!(e.scene().get(f).unwrap().kind.tag(), KindTag::Function);
    // A plot node whose text reads as a definition under `=`.
    let p = plot(&mut e, "k = 3");
    assert!(e.scene().status(p).is_error());
}
