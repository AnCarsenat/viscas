//! Reference extraction: bound variables, implicit plot variables, splits.

use super::*;

fn v(expr: &str) -> Vec<String> {
    refs(&NodeContent::expression(expr))
}

fn p(relation: &str) -> Vec<String> {
    refs(&NodeContent::plot(relation))
}

fn s(src: &str) -> Vec<String> {
    refs(&NodeContent::script(ScriptLanguage::Viscas, src))
}

fn names(list: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = list.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

#[test]
fn simple_expression_refs() {
    assert_eq!(v("a + b * c"), names(&["a", "b", "c"]));
    assert_eq!(v("2 + 3"), names(&[]));
    assert_eq!(v("a + a"), names(&["a"]));
}

#[test]
fn calls_reference_the_callee() {
    assert_eq!(v("f(a) + sin x"), names(&["a", "f", "sin", "x"]));
    assert_eq!(v("f'(2)"), names(&["f"]));
}

#[test]
fn member_access_references_only_the_object() {
    assert_eq!(v("P.x + T.x_1"), names(&["P", "T"]));
}

#[test]
fn function_params_are_bound() {
    let c = NodeContent::function("f", &["x", "y"], "x^2 + a y + g(x)");
    assert_eq!(refs(&c), names(&["a", "g"]));
}

#[test]
fn function_self_reference_is_kept() {
    let c = NodeContent::function("f", &["n"], "{n <= 1: 1, n f(n - 1)}");
    assert_eq!(refs(&c), names(&["f"]));
}

#[test]
fn comprehension_binds_sequentially() {
    assert_eq!(v("[k^2 + a for k = [1..n]]"), names(&["a", "n"]));
    assert_eq!(v("[k j for k = L, j = [1..k]]"), names(&["L"]));
    // The first binding's iterator cannot see a later binding.
    assert_eq!(v("[k for k = j, j = L]"), names(&["L", "j"]));
}

#[test]
fn sum_and_product_bind_their_index() {
    assert_eq!(v("∑(k = 1, n) k^2 a"), names(&["a", "n"]));
    assert_eq!(v("∏(i = a, b) i"), names(&["a", "b"]));
    // The index is free outside the body.
    assert_eq!(v("∑(k = 1, k) k"), names(&["k"]));
}

#[test]
fn derivative_and_integral_bind_their_variable() {
    assert_eq!(v("d/dx (x^2 a)"), names(&["a"]));
    assert_eq!(v("∫(0, b) t c dt"), names(&["b", "c"]));
    assert_eq!(v("∫ u du"), names(&[]));
    // Bounds are outside the binder.
    assert_eq!(v("∫(0, t) t dt"), names(&["t"]));
}

#[test]
fn piecewise_restrict_index_range() {
    assert_eq!(v("{a < 0: b, c}"), names(&["a", "b", "c"]));
    assert_eq!(v("L[i]"), names(&["L", "i"]));
    assert_eq!(v("[a, b..c]"), names(&["a", "b", "c"]));
    assert_eq!(v("(q {q > z})"), names(&["q", "z"]));
}

#[test]
fn plot_binds_x_and_y() {
    assert_eq!(p("y = a x^2 + b"), names(&["a", "b"]));
    assert_eq!(p("x^2 + y^2 < r_0^2"), names(&["r_0"]));
}

#[test]
fn plot_t_only_bound_for_parametric() {
    assert_eq!(p("(cos t, sin t)"), names(&["cos", "sin"]));
    assert_eq!(p("y = t x"), names(&["t"]));
}

#[test]
fn plot_r_only_bound_for_polar() {
    assert_eq!(p("r = 2θ"), names(&[]));
    assert_eq!(p("r = 2"), names(&[]));
    assert_eq!(p("x^2 + y^2 = r^2"), names(&["r"]));
    assert_eq!(p("y = r sin θ"), names(&["sin"]));
}

#[test]
fn plot_restrictions_are_walked() {
    assert_eq!(p("y = x {a < x < b}"), names(&["a", "b"]));
}

#[test]
fn x_is_free_outside_plots() {
    assert_eq!(v("x + 1"), names(&["x"]));
}

#[test]
fn regression_params_are_bound_and_defined() {
    let c = NodeContent::regression("y_1 ~ m x_1 + b", &["m", "b"]);
    assert_eq!(refs(&c), names(&["x_1", "y_1"]));
    assert_eq!(analyze(&c).defines, vec!["m", "b"]);
}

#[test]
fn action_targets_are_writes() {
    let a = analyze(&NodeContent::action("a -> a + b, c -> 0"));
    assert_eq!(a.referenced_names(), vec!["a", "b"]);
    assert_eq!(a.writes, vec!["a", "c"]);
}

#[test]
fn script_let_scoping() {
    assert_eq!(s("let q = a\nq + b"), names(&["a", "b"]));
    // `q` used before its `let` is free.
    assert_eq!(s("q + 1\nlet q = 2"), names(&["q"]));
}

#[test]
fn script_for_and_blocks() {
    assert_eq!(
        s("for k in L { print(k + c) }"),
        names(&["L", "c", "print"])
    );
    assert_eq!(s("if a { let z = 1 }\nz"), names(&["a", "z"]));
    assert_eq!(s("while n > 0 { n = n - 1 }"), names(&["n"]));
}

#[test]
fn script_fn_hoisting_and_params() {
    let c = NodeContent::script(
        ScriptLanguage::Viscas,
        "helper(2)\nfn helper(u) {\n  return u * k + helper(u - 1)\n}",
    );
    let a = analyze(&c);
    assert_eq!(a.referenced_names(), vec!["k"]);
    assert_eq!(a.defines, vec!["helper"]);
}

#[test]
fn script_assignment_is_a_write_and_local_functions_bind() {
    let a = analyze(&NodeContent::script(
        ScriptLanguage::Viscas,
        "a = b + 1\ng(x) = x + c\ng(2)",
    ));
    assert_eq!(a.referenced_names(), vec!["b", "c"]);
    assert_eq!(a.writes, vec!["a"]);
}

#[test]
fn rhai_scripts_are_opaque() {
    let a = analyze(&NodeContent::script(ScriptLanguage::Rhai, "let x = a + ;"));
    assert!(a.sources.is_empty());
    assert!(a.references.is_empty());
    assert!(!a.has_errors());
}

#[test]
fn parse_errors_have_spans() {
    let a = analyze(&NodeContent::variable("a", "1 + * 2"));
    assert!(a.has_errors());
    let (field, e) = a.first_error().unwrap();
    assert_eq!(field, SourceField::Main);
    assert!(e.span.start <= 4 && e.span.end <= 7);
    assert!(a.ast(SourceField::Main).is_none());
}

#[test]
fn script_partial_parse_keeps_good_statements() {
    let a = analyze(&NodeContent::script(ScriptLanguage::Viscas, "q + 1\n)\nw"));
    assert!(a.has_errors());
    assert!(a.referenced_names().contains(&"q"));
}

#[test]
fn blank_fields_are_skipped() {
    let a = analyze(&NodeContent::slider("s", "1", "", ""));
    assert!(!a.has_errors());
    assert_eq!(a.sources.len(), 4);
    assert!(a.ast(SourceField::Min).is_none());
    assert!(a.ast(SourceField::Main).is_some());
}

#[test]
fn slider_bounds_are_references() {
    let c = NodeContent::slider("s", "1", "lo", "hi + 1");
    assert_eq!(refs(&c), names(&["hi", "lo"]));
    let a = analyze(&c);
    assert!(a.references.iter().any(|r| r.field == SourceField::Min));
}

#[test]
fn table_cells_and_formulas() {
    let mut col = Column::new("y_1", &["1", "a", ""]);
    col.formula = None;
    let mut col2 = Column::new("y_2", &[]);
    col2.formula = Some("b x_1".into());
    let c = NodeContent::table("T", vec![col, col2]);
    let a = analyze(&c);
    assert_eq!(a.referenced_names(), vec!["a", "b", "x_1"]);
    assert!(
        a.references
            .iter()
            .any(|r| r.field == SourceField::Cell { column: 0, row: 1 })
    );
    assert!(
        a.references
            .iter()
            .any(|r| r.field == SourceField::Formula { column: 1 })
    );
}

#[test]
fn defines_follow_kind() {
    assert_eq!(analyze(&NodeContent::variable("a", "1")).defines, vec!["a"]);
    assert!(analyze(&NodeContent::expression("1")).defines.is_empty());
    assert!(
        analyze(&NodeContent::plot("y = x").named("a"))
            .defines
            .is_empty()
    );
    assert!(analyze(&NodeContent::folder("F")).defines.is_empty());
    assert_eq!(analyze(&NodeContent::table("T", vec![])).defines, vec!["T"]);
}

#[test]
fn reference_spans_point_at_identifiers() {
    let src = "alpha + f(beta)";
    let a = analyze(&NodeContent::expression(src));
    for r in &a.references {
        assert_eq!(&src[r.span.start..r.span.end], r.name);
    }
    assert_eq!(a.references.len(), 3);
}

#[test]
fn multi_letter_names_carry_splits() {
    let a = analyze(&NodeContent::plot("y = ax"));
    let r = a.references.iter().find(|r| r.name == "ax").unwrap();
    let split = r.split.as_ref().unwrap();
    assert_eq!(split.len(), 2);
    assert!(!split[0].bound, "a is free");
    assert!(split[1].bound, "x is a plot variable");
    let a = analyze(&NodeContent::expression("x_1 + a1 + q"));
    assert!(a.references.iter().all(|r| r.split.is_none()));
}

#[test]
fn polar_r_marks_split_letters_bound() {
    let a = analyze(&NodeContent::plot("r = kr θ"));
    let r = a.references.iter().find(|r| r.name == "kr").unwrap();
    let split = r.split.as_ref().unwrap();
    assert!(!split[0].bound);
    assert!(split[1].bound);
}

#[test]
fn ast_is_cached_in_scene() {
    let mut sc = Scene::new();
    let a = var(&mut sc, "a", "1 + 2");
    let an = sc.analysis(a).unwrap();
    assert!(matches!(an.ast(SourceField::Main), Some(Parsed::Expr(_))));
    let p = plot(&mut sc, "y = a x");
    assert!(matches!(
        sc.analysis(p).unwrap().ast(SourceField::Main),
        Some(Parsed::Stmt(_))
    ));
}
