//! Evaluator-level tests of the CAS integration.

use super::*;

fn ev(src: &str) -> String {
    let mut s = Session::new();
    match s.eval(src) {
        Ok(v) => v.to_string(),
        Err(e) => panic!("`{src}` failed: {e}"),
    }
}

fn ev_mode(src: &str, f: impl FnOnce(&mut EvalOptions)) -> String {
    let mut s = Session::new();
    f(&mut s.opts);
    match s.eval(src) {
        Ok(v) => v.to_string(),
        Err(e) => panic!("`{src}` failed: {e}"),
    }
}

fn err(src: &str) -> EvalError {
    match Session::new().eval(src) {
        Ok(v) => panic!("`{src}` should fail, got {v}"),
        Err(e) => e,
    }
}

macro_rules! cases {
    ($($name:ident: $src:expr => $want:expr;)*) => {
        $(
            #[test]
            fn $name() {
                assert_eq!(ev($src), $want, "source: {}", $src);
            }
        )*
    };
}

#[test]
fn catalog_covers_every_builtin() {
    assert!(
        catalog::missing_docs().is_empty(),
        "{:?}",
        catalog::missing_docs()
    );
    for (alias, _) in ALIASES {
        assert!(function_doc(alias).is_some(), "alias {alias}");
    }
}

#[test]
fn catalog_examples_hold() {
    for doc in catalog() {
        for (input, want) in doc.examples {
            assert_eq!(&ev(input), want, "example of {}: {input}", doc.name);
        }
        assert!(
            !doc.signatures.is_empty() && !doc.summary.is_empty(),
            "{}",
            doc.name
        );
    }
}

cases! {
    // exact constants
    pi_is_exact: "π" => "π";
    pi_times: "2π" => "2π";
    pi_sum: "π + π/2" => "3π/2";
    sin_pi_zero: "sin(π)" => "0";
    cos_two_pi: "cos(2π)" => "1";
    sin_pi_third: "sin(π/3)" => "√3/2";
    tan_pi_quarter: "tan(π/4)" => "1";
    cos_5pi_6: "cos(5π/6)" => "-√3/2";
    e_exact: "e" => "e";
    e_squared: "e^2" => "e^2";
    exp_ln: "simplify(exp(ln(5)))" => "5";
    ln_e_cubed: "ln(e^3)" => "3";
    tau_half: "τ/2" => "π";
    phi_exact: "φ" => "√5/2 + 1/2";
    phi_identity: "φ^2 - φ - 1 == 0" => "true";
    sqrt2_exact: "sqrt(2)" => "√2";
    sqrt2_squared: "sqrt(2)^2" => "2";
    sqrt_product: "sqrt(2) sqrt(8)" => "4";
    surd_power: "2^(1/2)" => "√2";
    cbrt_neg: "(-2)^(1/3)" => "-∛2";
    surd_sum: "√2 + √2" => "2√2";
    surd_div: "1/√2" => "√2/2";
    pi_compare: "π > 3" => "true";
    pi_compare2: "π < 22/7" => "true";
    pi_eq_num: "π == num(π)" => "true";
    sin_one_numeric: "sin(1)" => "≈0.841470984808";
    ln_two_numeric: "ln(2)" => "≈0.69314718056";
    float_contagion: "π + 0.5" => "π + 1/2";
    float_real: "π * 1.5" => "3π/2";
    approx_pi: "num(2π)" => "≈6.28318530718";
    exact_keeps_expr: "exact(π)" => "π";
    list_of_exprs: "[π, √2]" => "[π, √2]";
    sqrt_list: "sqrt([2, 4])" => "[√2, 2]";
    range_with_pi: "len([0, π/4 .. π])" => "5";
    round_pi: "round(π)" => "3";
    floor_sqrt: "floor(√50)" => "7";
    point_pi: "(π, 1)" => "(π, 1)";
    sum_sym: "sum([π, 1])" => "π + 1";
    // degree mode is checked below
    // CAS functions
    simplify_trig: "simplify(sin(x)^2 + cos(x)^2)" => "1";
    simplify_frac: "simplify((x^2 - 4)/(x - 2))" => "x + 2";
    simplify_logs: "simplify(ln(2) + ln(3) - ln(6))" => "0";
    expand_binomial: "expand((a + b)^2)" => "a^2 + 2a·b + b^2";
    expand_with_global: "a := 2; expand((x + a)^2)" => "x^2 + 4x + 4";
    factor_poly: "factor(x^2 - 5x + 6)" => "(x - 3)(x - 2)";
    factor_int: "factor(360)" => "[2, 2, 2, 3, 3, 5]";
    together_sum: "together(1/(x - 1) + 1/(x + 1))" => "2x/((x - 1)(x + 1))";
    cancel_sum: "cancel(1/(x - 1) + 1/(x + 1))" => "2x/(x^2 - 1)";
    collect_x: "collect(x^2 + a x^2 + x, x)" => "(a + 1)·x^2 + x";
    coeffs_q: "coeffs(3x^2 - x + 5, x)" => "[3, -1, 5]";
    degree_q: "degree(x^5 - x, x)" => "5";
    partfrac_q: "partfrac((2x + 1)/(x^2 + x - 2), x)" => "1/(x - 1) + 1/(x + 2)";
    numerator_q: "numerator(x/(x + 1))" => "x";
    diff_poly: "diff(x^3 + 2x, x)" => "3x^2 + 2";
    diff_trig: "diff(sin(x) cos(x), x)" => "-sin(x)^2 + cos(x)^2";
    diff_exp: "diff(e^(3x), x)" => "3e^(3x)";
    diff_second: "diff(sin(x), x, 2)" => "-sin(x)";
    diff_partial: "diff(x^2 y^3, x, y)" => "6x·y^2";
    diff_user_fn: "f(t) := t^3; diff(f(x), x)" => "3x^2";
    diff_fn_value: "g(t) := t^2 + 1; diff(g)" => "2x";
    diff_infer_var: "diff(x^2)" => "2x";
    diff_ln: "diff(ln(x^2 + 1), x)" => "2x/(x^2 + 1)";
    d_dx_shadow_local: "let x = 3; d/dx x^2" => "6";
    prime_exact: "f(x) := x^3; f'(2)" => "12";
    prime_symbolic: "f(x) := sin(x); f'(π/3)" => "1/2";
    subs_value: "subs(x^2 + y, x, 2)" => "y + 4";
    subs_eq: "subs(x^2 + y, [x = 1, y = 2])" => "3";
    solve_quadratic: "solve(x^2 - 5x + 6 = 0, x)" => "[2, 3]";
    solve_infer: "solve(2x + 1 = 0)" => "[-1/2]";
    solve_surd: "solve(x^2 = 3, x)" => "[-√3, √3]";
    solve_exp: "solve(e^x = 5, x)" => "[ln(5)]";
    solve_symbolic: "solve(a x + b = 0, x)" => "[-b/a]";
    solve_with_global: "c := 4; solve(x^2 = c, x)" => "[-2, 2]";
    solve_trig_domain: "solve(cos(x) = 0, x, 0, 2π)" => "[π/2, 3π/2]";
    solve_trig_general: "solve(sin(x) = 0, x)" => "[2π·k, 2π·k + π]";
    solve_none: "solve(x^2 = -1, x)" => "[]";
    solve_all: "solve(x + 1 = x + 1, x)" => "true";
    solve_ineq: "solve(x^2 - 1 > 0, x)" => "[x < -1, x > 1]";
    solve_ineq_abs: "solve(abs(x) <= 2, x)" => "[-2 ≤ x ≤ 2]";
    solve_system: "solve([2x + y = 5, x - y = 1], [x, y])" => "[[x = 2, y = 1]]";
    solve_system_nonlinear: "solve([x^2 + y^2 = 2, x = y], [x, y])" => "[[x = -1, y = -1], [x = 1, y = 1]]";
    solve_y_plot_var: "solve(y^2 = 4, y)" => "[-2, 2]";
    roots_cubic: "roots(x^3 - 3x^2 + 3x - 1)" => "[1, 1, 1]";
    nsolve_interval: "len(nsolve(sin(x), x, -1, 4))" => "2";
    lists_map: "simplify([x + x, x*x])" => "[2x, x^2]";
}

#[test]
fn complex_mode_solutions() {
    assert_eq!(
        ev_mode("solve(x^2 + 4 = 0, x)", |o| o.complex = true),
        "[-2i, 2i]"
    );
    assert_eq!(ev_mode("roots(x^2 + 1)", |o| o.complex = true), "[-i, i]");
    assert_eq!(ev_mode("sqrt(-2)", |o| o.complex = true), "i·√2");
}

#[test]
fn degree_mode_exact() {
    let deg = |o: &mut EvalOptions| o.angle = AngleUnit::Degrees;
    assert_eq!(ev_mode("sin(45)", deg), "√2/2");
    assert_eq!(ev_mode("cos(60)", deg), "1/2");
    assert_eq!(ev_mode("asin(1/2)", deg), "30");
    assert_eq!(ev_mode("tan(30)", deg), "√3/3");
}

#[test]
fn approx_mode_shows_decimals() {
    let run = |src: &str| {
        let mut s = Session::new();
        s.opts.approx = true;
        s.eval_to_string(src).unwrap()
    };
    assert_eq!(run("π"), "3.14159265359");
    assert_eq!(run("sqrt(2)"), "1.41421356237");
    assert_eq!(run("diff(x^2 + π x, x)"), "2x + 3.14159265359");
}

#[test]
fn float64_mode_stays_numeric() {
    let mut s = Session::new();
    s.opts.float64 = true;
    let v = s.eval("π").unwrap();
    assert!(matches!(v, Value::Number(_)));
    let v = s.eval("sqrt(2)").unwrap();
    assert!(matches!(v, Value::Number(_)));
}

#[test]
fn real_mode_errors_stay() {
    assert_eq!(err("sqrt(-1)").kind, ErrorKind::NotReal);
    assert_eq!(err("ln(-1)").kind, ErrorKind::NotReal);
    assert!(matches!(err("1/0").kind, ErrorKind::Number(_)));
    assert!(matches!(err("tan(π/2)").kind, ErrorKind::Number(_)));
}

#[test]
fn unknown_names_outside_cas_are_errors() {
    assert!(matches!(err("q + 1").kind, ErrorKind::UnknownName(_)));
    assert!(err("solve(x^2 = 4, x) + q").message.contains("q"));
}

#[test]
fn steps_function() {
    let v = ev("steps(diff(sin(x^2), x))");
    assert!(v.contains("chain rule"), "{v}");
    let v = ev("steps(solve(x^2 - 2x - 1 = 0, x))");
    assert!(v.contains("quadratic formula"), "{v}");
    let v = ev("steps(factor(x^2 - 1))");
    assert!(v.contains("factor"), "{v}");
}

#[test]
fn session_steps_api() {
    let mut s = Session::new();
    let (v, steps) = s.eval_with_steps("simplify(sin(x)^2 + cos(x)^2)").unwrap();
    assert_eq!(v.to_string(), "1");
    assert!(!steps.is_empty());
    assert!(
        steps.iter().any(|st| st.rule.contains("Pythagorean")),
        "{steps:?}"
    );
}

#[test]
fn as_number_of_constants() {
    let v = Session::new().eval("π").unwrap();
    assert!((v.as_number().unwrap().to_f64() - std::f64::consts::PI).abs() < 1e-15);
    assert!((v.to_f64() - std::f64::consts::PI).abs() < 1e-15);
    let v = Session::new().eval("diff(x^2, x)").unwrap();
    assert!(v.as_number().is_none());
    assert_eq!(v.type_name(), "expression");
}

#[test]
fn classify_cas_call_has_no_free_vars() {
    let stmt = crate::syntax::parse_statement("solve(x^2 = 4, x)").unwrap();
    let info = classify(&stmt, &Env::new());
    assert!(info.free_vars.is_empty(), "{:?}", info.free_vars);
    assert_eq!(info.kind, StatementKind::Expression);
}
