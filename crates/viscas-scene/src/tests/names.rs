//! Identifier validation, builtins and line classification.

use super::*;

#[test]
fn valid_identifiers() {
    for ok in ["a", "abc", "x1", "x_1", "x_ab", "θ", "π", "km", "Ab"] {
        assert!(is_valid_identifier(ok), "{ok}");
    }
}

#[test]
fn invalid_identifiers() {
    for bad in [
        "", "1", "1a", "a b", "a+b", "for", "let", "and", "mod", "to", "x_{1}", "a.b", "a,", "_a",
        "a\n", "θa",
    ] {
        assert!(!is_valid_identifier(bad), "{bad:?}");
    }
}

#[test]
fn default_builtins() {
    let b = DefaultBuiltins;
    for name in [
        "sin", "cos", "ln", "sqrt", "pi", "π", "e", "mean", "log_2", "solve",
    ] {
        assert!(b.is_builtin(name), "{name}");
    }
    for name in ["a", "f", "log_", "foo"] {
        assert!(!b.is_builtin(name), "{name}");
    }
}

#[test]
fn closure_builtins() {
    let b = |n: &str| n == "km";
    assert!(Builtins::is_builtin(&b, "km"));
    assert!(!Builtins::is_builtin(&b, "m"));
}

#[test]
fn unique_name_free_function() {
    let taken = ["f", "f_2"];
    assert_eq!(unique_name("f", |n| taken.contains(&n)), "f_3");
    assert_eq!(unique_name("g", |n| taken.contains(&n)), "g");
}

fn classify(line: &str) -> NodeContent {
    parse_definition(line).unwrap()
}

#[test]
fn classify_variable_point_list() {
    assert_eq!(classify("a := 3"), NodeContent::variable("a", "3"));
    assert_eq!(classify("P := (1, 2)"), NodeContent::point("P", "(1, 2)"));
    assert_eq!(classify("L := [1..10]"), NodeContent::list("L", "[1..10]"));
    assert_eq!(
        classify("S := [k^2 for k = [1..5]]"),
        NodeContent::list("S", "[k^2 for k = [1..5]]")
    );
}

#[test]
fn classify_function() {
    assert_eq!(
        classify("f(x, y) := x^2 + a y"),
        NodeContent::function("f", &["x", "y"], "x^2 + a y")
    );
    assert_eq!(
        classify("g(t) := t {t > 0}"),
        NodeContent::function("g", &["t"], "t {t > 0}")
    );
}

#[test]
fn equals_is_a_relation_by_default() {
    for line in ["a = 3", "f(x) = x^2", "P = (1, 2)"] {
        assert_eq!(classify(line).kind.tag(), KindTag::Plot, "{line}");
    }
}

#[test]
fn eq_mode_defines_with_equals() {
    let opts = LineOptions {
        definition_op: DefinitionOp::Eq,
    };
    let c = |l: &str| parse_definition_with(l, opts).unwrap();
    assert_eq!(c("a = 3"), NodeContent::variable("a", "3"));
    assert_eq!(c("f(x) = x^2"), NodeContent::function("f", &["x"], "x^2"));
    assert_eq!(c("b := 1"), NodeContent::variable("b", "1"));
    for line in ["y = sin x", "x = 3", "r = θ", "θ = 1", "sin(x) = y"] {
        assert_eq!(c(line).kind.tag(), KindTag::Plot, "{line}");
    }
    let f = NodeContent::function("f", &["x"], "x^2");
    assert_eq!(
        definition_line_with(&f, opts).as_deref(),
        Some("f(x) = x^2")
    );
    assert_eq!(definition_line(&f).as_deref(), Some("f(x) := x^2"));
}

#[test]
fn classify_plots() {
    for line in [
        "y = sin x",
        "x = 3",
        "r = θ",
        "x^2 + y^2 = 4",
        "x^2 + y^2 < 4",
        "(cos t, sin t)",
        "y = x {0 < x < 2}",
        "f(2) = 3",
    ] {
        assert_eq!(classify(line).kind.tag(), KindTag::Plot, "{line}");
    }
}

#[test]
fn classify_regression_action_expression() {
    assert_eq!(classify("y_1 ~ m x_1 + b").kind.tag(), KindTag::Regression);
    assert_eq!(classify("a -> a + 1").kind.tag(), KindTag::Action);
    assert_eq!(classify("2 + 3"), NodeContent::expression("2 + 3"));
    assert_eq!(classify("let q = 2").kind.tag(), KindTag::Script);
}

#[test]
fn classify_reports_parse_errors() {
    let errs = parse_definition("a = (1 +").unwrap_err();
    assert!(!errs.is_empty());
}

#[test]
fn definition_line_roundtrip() {
    for line in [
        "a := 3",
        "f(x, y) := x y",
        "y = sin x",
        "P := (1, 2)",
        "a -> a + 1",
    ] {
        let c = classify(line);
        assert_eq!(definition_line(&c).as_deref(), Some(line));
    }
    assert_eq!(definition_line(&NodeContent::folder("F")), None);
}
