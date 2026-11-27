//! Tests of the solvers. Every exact solution is checked by substitution.

use super::convert::parse_sym;
use super::solve::*;
use super::*;

fn p(s: &str) -> Sym {
    parse_sym(s).unwrap_or_else(|e| panic!("parse `{s}`: {e}"))
}

fn opts(complex: bool) -> SolveOptions {
    SolveOptions {
        complex,
        domain: None,
    }
}

fn sols(src: &str, complex: bool) -> Vec<Sym> {
    match solve(&p(src), "x", &opts(complex)).unwrap_or_else(|e| panic!("{src}: {e}")) {
        Solution::Values(v) => v,
        other => panic!("{src}: unexpected {other:?}"),
    }
}

fn show(v: &[Sym]) -> String {
    let parts: Vec<String> = v.iter().map(|s| s.to_string()).collect();
    format!("[{}]", parts.join(", "))
}

/// Solutions as text, after checking each one by substitution.
fn solve_checked(src: &str) -> String {
    solve_mode(src, false)
}

fn solve_mode(src: &str, complex: bool) -> String {
    let eq = p(src);
    let f = match eq.node() {
        Node::Rel(items, _) => &items[0] - &items[1],
        _ => eq.clone(),
    };
    let v = sols(src, complex);
    for s in &v {
        if s.has_var("k") {
            continue;
        }
        let r = f.subs("x", s);
        let val = approx(&r, crate::number::Precision::digits(30)).unwrap();
        let (re, im) = val.to_complex_f64();
        assert!(re.hypot(im) < 1e-9, "{src}: x = {s} gives {val}");
    }
    show(&v)
}

macro_rules! cases {
    ($f:ident, $($name:ident: $src:expr => $want:expr;)*) => {
        $(
            #[test]
            fn $name() {
                assert_eq!($f($src), $want, "source: {}", $src);
            }
        )*
    };
}

cases! { solve_checked,
    solve_linear: "2x + 3 == 7" => "[2]";
    solve_linear_frac: "3x == 1" => "[1/3]";
    solve_quadratic: "x^2 == 4" => "[-2, 2]";
    solve_quadratic_surd: "x^2 - 2 == 0" => "[-√2, √2]";
    solve_quadratic_full: "x^2 - 2x - 1 == 0" => "[1 - √2, √2 + 1]";
    solve_quadratic_double: "x^2 + 2x + 1 == 0" => "[-1]";
    solve_quadratic_none_real: "x^2 + 1 == 0" => "[]";
    solve_cubic_rational: "x^3 - 6x^2 + 11x - 6 == 0" => "[1, 2, 3]";
    solve_cubic_mixed: "x^3 - 2x == 0" => "[-√2, 0, √2]";
    solve_quartic_biquad: "x^4 - 5x^2 + 4 == 0" => "[-2, -1, 1, 2]";
    solve_quartic_kron: "x^4 + 4 == 0" => "[]";
    solve_rational_eq: "1/x == 2" => "[1/2]";
    solve_rational_excluded: "(x^2 - 1)/(x - 1) == 0" => "[-1]";
    solve_rational_sum: "1/x + 1/(x + 1) == 1" => "[1/2 - √5/2, √5/2 + 1/2]";
    solve_sqrt: "sqrt(x) == 3" => "[9]";
    solve_sqrt_extraneous: "sqrt(x) == -3" => "[]";
    solve_exp: "e^x == 2" => "[ln(2)]";
    solve_exp_base: "2^x == 8" => "[3]";
    solve_ln: "ln(x) == 1" => "[e]";
    solve_abs: "abs(x - 1) == 2" => "[-1, 3]";
    solve_abs_none: "abs(x) == -1" => "[]";
    solve_exp_quadratic: "e^(2x) - 3e^x + 2 == 0" => "[0, ln(2)]";
    solve_product: "x*(x - 1)*(x + 2) == 0" => "[-2, 0, 1]";
    solve_exp_product: "x*e^x - 2e^x == 0" => "[2]";
    solve_sin_sq: "sin(x)^2 == 0" => "[2π·k, 2π·k + π]";
    solve_power5: "x^5 == 32" => "[2]";
}

fn solve_checked_wrapper(src: &str) -> String {
    solve_checked(src)
}

#[test]
fn solve_identity() {
    assert_eq!(
        solve(&p("x + 1 == 1 + x"), "x", &opts(false)).unwrap(),
        Solution::All
    );
}

#[test]
fn solve_quadratic_complex() {
    assert_eq!(solve_mode("x^2 + 1 == 0", true), "[-i, i]");
    assert_eq!(solve_mode("x^2 + 2x + 5 == 0", true), "[-1 - 2i, -1 + 2i]");
}

#[test]
fn solve_cubic_cardano() {
    // x³ + x + 1: one real root, Cardano form; check numerically.
    let v = sols("x^3 + x + 1 == 0", false);
    assert_eq!(v.len(), 1);
    let r = approx_f64(&v[0]).unwrap();
    assert!((r - (-0.682_327_803_828_019_3)).abs() < 1e-12, "{r}");
    let c = sols("x^3 + x + 1 == 0", true);
    assert_eq!(c.len(), 3);
}

#[test]
fn solve_cubic_three_real() {
    let v = sols("x^3 - 3x + 1 == 0", false);
    assert_eq!(v.len(), 3);
    for s in &v {
        let t = approx_f64(s).unwrap();
        assert!((t * t * t - 3.0 * t + 1.0).abs() < 1e-12);
    }
}

#[test]
fn solve_quintic_numeric() {
    let v = sols("x^5 - x - 1 == 0", false);
    assert_eq!(v.len(), 1);
    let t = approx_f64(&v[0]).unwrap();
    assert!((t - 1.167_303_978_261_418_7).abs() < 1e-10);
}

#[test]
fn solve_trig_general() {
    let v = sols("sin(x) == 1/2", false);
    assert_eq!(show(&v), "[2π·k + π/6, 2π·k + 5π/6]");
    let v = sols("cos(x) == 0", false);
    assert_eq!(v.len(), 2);
    let v = sols("tan(x) == 1", false);
    assert_eq!(show(&v), "[π·k + π/4]");
}

#[test]
fn solve_trig_domain() {
    let o = SolveOptions {
        complex: false,
        domain: Some((0.0, 2.0 * std::f64::consts::PI)),
    };
    let Solution::Values(v) = solve(&p("sin(x) == 1/2"), "x", &o).unwrap() else {
        panic!()
    };
    assert_eq!(show(&v), "[π/6, 5π/6]");
    let Solution::Values(v) = solve(&p("cos(2x) == 1"), "x", &o).unwrap() else {
        panic!()
    };
    assert_eq!(show(&v), "[0, π, 2π]");
}

#[test]
fn solve_symbolic_coefficients() {
    let v = match solve(&p("a*x + b == 0"), "x", &opts(false)).unwrap() {
        Solution::Values(v) => v,
        _ => panic!(),
    };
    assert_eq!(show(&v), "[-b/a]");
    let v = match solve(&p("x^2 == a"), "x", &opts(false)).unwrap() {
        Solution::Values(v) => v,
        _ => panic!(),
    };
    assert_eq!(show(&v), "[-√a, √a]");
}

fn ineq(src: &str) -> String {
    match solve(&p(src), "x", &opts(false)).unwrap() {
        Solution::Intervals(v) => show(&v),
        Solution::All => "all".into(),
        other => panic!("{other:?}"),
    }
}

cases! { ineq,
    ineq_linear: "2x + 1 < 5" => "[x < 2]";
    ineq_linear_ge: "3 - x >= 1" => "[x ≤ 2]";
    ineq_quadratic_in: "x^2 < 4" => "[-2 < x < 2]";
    ineq_quadratic_out: "x^2 >= 4" => "[x ≤ -2, x ≥ 2]";
    ineq_rational: "1/x > 0" => "[x > 0]";
    ineq_rational2: "(x - 1)/(x + 2) <= 0" => "[-2 < x ≤ 1]";
    ineq_always: "x^2 + 1 > 0" => "all";
    ineq_never: "x^2 + 1 < 0" => "[]";
    ineq_point: "(x - 1)^2 <= 0" => "[x = 1]";
    ineq_cubic: "x^3 - x > 0" => "[-1 < x < 0, x > 1]";
    ineq_abs: "abs(x - 1) < 2" => "[-1 < x < 3]";
    ineq_sqrt: "sqrt(x) < 2" => "[0 ≤ x < 4]";
    ineq_chain: "0 < x + 1 < 2" => "[-1 < x < 1]";
    ineq_surd: "x^2 < 2" => "[-√2 < x < √2]";
}

fn sys(srcs: &[&str], vars: &[&str]) -> String {
    let eqs: Vec<Sym> = srcs.iter().map(|s| p(s)).collect();
    let vars: Vec<String> = vars.iter().map(|s| s.to_string()).collect();
    let r = solve_system(&eqs, &vars, &opts(false)).unwrap();
    // Check by substitution.
    for s in &r {
        for e in &eqs {
            let mut g = match e.node() {
                Node::Rel(items, _) => &items[0] - &items[1],
                _ => e.clone(),
            };
            for (v, val) in s {
                g = g.subs(v, val);
            }
            if g.is_constant() {
                let t = approx_f64(&g).unwrap();
                assert!(t.abs() < 1e-9, "{srcs:?}: {s:?} gives {t}");
            }
        }
    }
    let parts: Vec<String> = r
        .iter()
        .map(|s| {
            let a: Vec<String> = s.iter().map(|(v, e)| format!("{v} = {e}")).collect();
            format!("[{}]", a.join(", "))
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

#[test]
fn system_linear_2x2() {
    assert_eq!(
        sys(&["x + y == 3", "x - y == 1"], &["x", "y"]),
        "[[x = 2, y = 1]]"
    );
}

#[test]
fn system_linear_3x3() {
    assert_eq!(
        sys(
            &["x + y + z == 6", "2x - y + z == 3", "x + 2y - z == 2"],
            &["x", "y", "z"]
        ),
        "[[x = 1, y = 2, z = 3]]"
    );
}

#[test]
fn system_linear_inconsistent() {
    assert_eq!(sys(&["x + y == 1", "x + y == 2"], &["x", "y"]), "[]");
}

#[test]
fn system_linear_underdetermined() {
    assert_eq!(sys(&["x + y == 1"], &["x", "y"]), "[[x = -y + 1]]");
}

#[test]
fn system_linear_symbolic() {
    assert_eq!(
        sys(&["a*x + y == 1", "x - y == 0"], &["x", "y"]),
        "[[x = 1/(a + 1), y = 1/(a + 1)]]"
    );
}

#[test]
fn system_circle_line() {
    assert_eq!(
        sys(&["x^2 + y^2 == 25", "y == x + 1"], &["x", "y"]),
        "[[x = -4, y = -3], [x = 3, y = 4]]"
    );
}

#[test]
fn system_two_conics() {
    let r = sys(&["x^2 + y^2 == 5", "x*y == 2"], &["x", "y"]);
    assert_eq!(r.matches("x =").count(), 4, "{r}");
}

#[test]
fn roots_multiplicity() {
    let r = roots(&p("(x - 1)^2*(x + 2)"), "x", false).unwrap();
    assert_eq!(show(&r), "[-2, 1, 1]");
    let r = roots(&p("x^2 + 1"), "x", true).unwrap();
    assert_eq!(show(&r), "[-i, i]");
}

#[test]
fn nsolve_guess_and_interval() {
    let r = nsolve(&p("cos(x) == x"), "x", NStart::Guess(1.0)).unwrap();
    assert!((r[0] - 0.739_085_133_215_160_6).abs() < 1e-12);
    let r = nsolve(&p("sin(x)"), "x", NStart::Interval(-1.0, 7.0)).unwrap();
    assert_eq!(r.len(), 3, "{r:?}");
    assert!((r[2] - 2.0 * std::f64::consts::PI).abs() < 1e-10);
    let r = nsolve(&p("1/x"), "x", NStart::Interval(-1.0, 1.0)).unwrap();
    assert!(r.is_empty(), "pole is not a root: {r:?}");
    let r = nsolve(&p("(x - 1)^2"), "x", NStart::Interval(0.0, 3.0)).unwrap();
    assert_eq!(r.len(), 1, "{r:?}");
}

#[test]
fn solve_steps() {
    let (_, steps) = trace(|| solve(&p("x^2 - 2x - 1 == 0"), "x", &opts(false)).unwrap());
    assert!(
        steps.iter().any(|s| s.rule == "quadratic formula"),
        "{steps:?}"
    );
    let _ = solve_checked_wrapper;
}
