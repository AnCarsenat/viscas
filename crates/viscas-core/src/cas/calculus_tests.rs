//! Tests of integration (checked by differentiation), limits and series
//! (checked numerically) and closed-form sums/products (checked against
//! explicit loops).

use super::convert::parse_sym;
use super::limit::{Dir, Lim, Point, infinity, limit};
use super::*;

fn p(s: &str) -> Sym {
    parse_sym(s).unwrap_or_else(|e| panic!("parse `{s}`: {e}"))
}

/// ∫ f dx found, and d/dx of it equals f at several points.
fn check_integral(src: &str) -> String {
    let f = p(src);
    let fi = integrate(&f, "x").unwrap_or_else(|| panic!("no antiderivative for {src}"));
    let d = diff(&fi, "x").unwrap();
    for x0 in [0.31, 0.77, 1.3, 2.2] {
        let a = eval_f64(&f, "x", x0);
        let b = eval_f64(&d, "x", x0);
        if a.is_finite() {
            assert!(
                (a - b).abs() <= 1e-8 * a.abs().max(1.0),
                "∫ {src} = {fi}: at {x0} f = {a}, F' = {b}"
            );
        }
    }
    fi.to_string()
}

macro_rules! int_cases {
    ($($name:ident: $src:expr;)*) => {
        $(
            #[test]
            fn $name() {
                check_integral($src);
            }
        )*
    };
}

int_cases! {
    in_const: "5";
    in_poly: "3x^2 + 2x + 1";
    in_power_neg: "x^(-3)";
    in_recip: "1/x";
    in_sqrt: "sqrt(x)";
    in_linear_power: "(2x + 1)^5";
    in_linear_recip: "1/(3x - 1)";
    in_exp: "e^x";
    in_exp_linear: "e^(3x + 1)";
    in_two_pow: "2^x";
    in_sin: "sin(x)";
    in_cos_linear: "cos(2x)";
    in_tan: "tan(x)";
    in_sinh: "sinh(x)";
    in_cosh: "cosh(3x)";
    in_tanh: "tanh(x)";
    in_ln: "ln(x)";
    in_atan: "atan(x)";
    in_asin: "asin(x/2)";
    in_sec2: "1/cos(x)^2";
    in_csc2: "1/sin(x)^2";
    in_arctan_form: "1/(1 + x^2)";
    in_arctan_scaled: "1/(4 + x^2)";
    in_asin_form: "1/sqrt(1 - x^2)";
    in_asinh_form: "1/sqrt(x^2 + 1)";
    in_partial: "1/(x^2 - 1)";
    in_partial2: "(x + 3)/(x^2 + 3x + 2)";
    in_partial_rep: "1/(x^2*(x + 1))";
    in_partial_quad: "(2x + 3)/(x^2 + 2x + 5)";
    in_improper: "(x^3 + 1)/(x - 2)";
    in_subst_chain: "2x*cos(x^2)";
    in_subst_exp: "x*e^(x^2)";
    in_subst_log: "ln(x)/x";
    in_subst_trig: "sin(x)*cos(x)";
    in_subst_root: "x*sqrt(x^2 + 1)";
    in_subst_frac: "cos(x)/(1 + sin(x))";
    in_parts_exp: "x*e^x";
    in_parts_poly_exp: "x^2*e^(2x)";
    in_parts_sin: "x*sin(x)";
    in_parts_cos2: "x^2*cos(x)";
    in_parts_log: "x*ln(x)";
    in_parts_log2: "x^2*ln(x)";
    in_parts_atan: "x*atan(x)";
    in_sin2: "sin(x)^2";
    in_cos2: "cos(x)^2";
    in_sin3: "sin(x)^3";
    in_cos4: "cos(x)^4";
    in_sin_cos_prod: "sin(3x)*cos(2x)";
    in_abs: "abs(x)";
    in_sum_mixed: "x^2 + sin(x) + e^(-x) + 1/x";
}

#[test]
fn integrate_display() {
    assert_eq!(check_integral("x^2"), "x^3/3");
    assert_eq!(check_integral("cos(x)"), "sin(x)");
    assert_eq!(check_integral("e^(2x)"), "e^(2x)/2");
    assert_eq!(check_integral("1/(1 + x^2)"), "atan(x)");
    assert_eq!(check_integral("x*e^x"), "e^x·(x - 1)");
}

#[test]
fn integrate_unknown_is_none() {
    assert!(integrate(&p("e^(x^2)"), "x").is_none());
    assert!(integrate(&p("sin(x)/x"), "x").is_none());
}

fn lim(src: &str, at: Point, dir: Dir) -> String {
    limit(&p(src), "x", &at, dir)
        .map(|l| l.to_sym().to_string())
        .unwrap_or_else(|e| format!("error: {}", e.0))
}

fn at(s: &str) -> Point {
    Point::Finite(p(s))
}

macro_rules! limit_cases {
    ($($name:ident: $src:expr, $at:expr, $dir:expr => $want:expr;)*) => {
        $(
            #[test]
            fn $name() {
                assert_eq!(lim($src, $at, $dir), $want, "lim {}", $src);
            }
        )*
    };
}

limit_cases! {
    li_direct: "x^2 + 1", at("2"), Dir::Both => "5";
    li_sinc: "sin(x)/x", at("0"), Dir::Both => "1";
    li_cos: "(1 - cos(x))/x^2", at("0"), Dir::Both => "1/2";
    li_exp: "(e^x - 1)/x", at("0"), Dir::Both => "1";
    li_rational_hole: "(x^2 - 1)/(x - 1)", at("1"), Dir::Both => "2";
    li_pole_even: "1/x^2", at("0"), Dir::Both => "∞";
    li_pole_odd_right: "1/x", at("0"), Dir::Plus => "∞";
    li_pole_odd_left: "1/x", at("0"), Dir::Minus => "-∞";
    li_pole_odd_both: "1/x", at("0"), Dir::Both => "error: the limit does not exist (the one-sided limits differ)";
    li_inf_rational: "(2x^2 + 1)/(x^2 - 3)", Point::PosInf, Dir::Both => "2";
    li_inf_rational_zero: "x/(x^2 + 1)", Point::PosInf, Dir::Both => "0";
    li_inf_poly: "x^3 - x", Point::NegInf, Dir::Both => "-∞";
    li_e: "(1 + 1/x)^x", Point::PosInf, Dir::Both => "e";
    li_exp_beats_poly: "x^5/e^x", Point::PosInf, Dir::Both => "0";
    li_exp_inf: "e^x/x^2", Point::PosInf, Dir::Both => "∞";
    li_xlnx: "x*ln(x)", at("0"), Dir::Plus => "0";
    li_ln_inf: "ln(x)/x", Point::PosInf, Dir::Both => "0";
    li_sqrt_diff: "sqrt(x^2 + x) - x", Point::PosInf, Dir::Both => "1/2";
    li_atan_inf: "atan(x)", Point::PosInf, Dir::Both => "π/2";
    li_exp_neg_inf: "e^x", Point::NegInf, Dir::Both => "0";
    li_tan_over_x: "tan(3x)/x", at("0"), Dir::Both => "3";
    li_x_pow_x: "x^x", at("0"), Dir::Plus => "1";
    li_ln_zero: "ln(x)", at("0"), Dir::Plus => "-∞";
    li_pi_point: "sin(x)/(x - π)", at("π"), Dir::Both => "-1";
}

/// The series agrees with the function near the point.
fn check_series(src: &str, a: f64, n: i64) -> String {
    let f = p(src);
    let s = series(
        &f,
        "x",
        &Sym::num(crate::number::Number::f64(a).unwrap()),
        n,
    );
    let s = s.unwrap_or_else(|| panic!("no series for {src}"));
    let h = 1e-2;
    let (fx, sx) = (eval_f64(&f, "x", a + h), eval_f64(&s, "x", a + h));
    assert!(
        (fx - sx).abs() < 100.0 * h.powi(n as i32),
        "{src}: {s} at {}: {fx} vs {sx}",
        a + h
    );
    s.to_string()
}

#[test]
fn series_known() {
    let s = |src: &str, n| series(&p(src), "x", &Sym::zero(), n).unwrap().to_string();
    assert_eq!(s("e^x", 4), "x^3/6 + x^2/2 + x + 1");
    assert_eq!(s("sin(x)", 6), "x^5/120 - x^3/6 + x");
    assert_eq!(s("cos(x)", 5), "x^4/24 - x^2/2 + 1");
    assert_eq!(s("ln(1 + x)", 4), "x^3/3 - x^2/2 + x");
    assert_eq!(s("1/(1 - x)", 4), "x^3 + x^2 + x + 1");
    assert_eq!(s("atan(x)", 6), "x^5/5 - x^3/3 + x");
    assert_eq!(s("sqrt(1 + x)", 3), "-x^2/8 + x/2 + 1");
    assert_eq!(s("tan(x)", 6), "2x^5/15 + x^3/3 + x");
    assert_eq!(s("sin(x)/x", 4), "-x^2/6 + 1");
    assert_eq!(s("1/sin(x)", 2), "x/6 + 1/x");
}

#[test]
fn series_at_point() {
    let s = series(&p("ln(x)"), "x", &Sym::one(), 3).unwrap();
    assert_eq!(s.to_string(), "-(x - 1)^2/2 + (x - 1)");
    let s = series(&p("e^x"), "x", &Sym::one(), 2).unwrap();
    assert!(s.to_string().contains("e"), "{s}");
}

macro_rules! series_cases {
    ($($name:ident: $src:expr, $a:expr, $n:expr;)*) => {
        $(
            #[test]
            fn $name() {
                check_series($src, $a, $n);
            }
        )*
    };
}

series_cases! {
    se_exp_sin: "e^(sin(x))", 0.0, 5;
    se_cosh: "cosh(x)", 0.0, 6;
    se_asin: "asin(x)", 0.0, 6;
    se_atanh: "atanh(x)", 0.0, 6;
    se_ln_cos: "ln(cos(x))", 0.0, 6;
    se_rational: "(1 + x)/(1 - 2x)", 0.0, 5;
    se_pow: "(1 + x)^(1/3)", 0.0, 5;
    se_shifted: "sin(x)", 1.0, 5;
    se_sinh_tanh: "sinh(x)*tanh(x)", 0.0, 6;
}

fn sum_check(src: &str, a: i64) {
    let f = p(src);
    let n = Sym::var("n");
    let s = summation(&f, "k", &Sym::int(a), &n).unwrap_or_else(|| panic!("no closed form {src}"));
    for nn in a..a + 6 {
        let want: Sym = Sym::add((a..=nn).map(|k| f.subs("k", &Sym::int(k))).collect());
        let got = simplify(&s.subs("n", &Sym::int(nn)));
        assert_eq!(simplify(&want), got, "Σ {src} at n = {nn}: {s}");
    }
}

macro_rules! sum_cases {
    ($($name:ident: $src:expr, $a:expr;)*) => {
        $(
            #[test]
            fn $name() {
                sum_check($src, $a);
            }
        )*
    };
}

sum_cases! {
    su_const: "3", 1;
    su_linear: "k", 1;
    su_square: "k^2", 1;
    su_cube: "k^3", 1;
    su_poly: "2k^2 - k + 5", 0;
    su_geometric: "2^k", 0;
    su_geometric_frac: "3*(1/2)^k", 1;
    su_telescoping: "1/(k*(k + 1))", 1;
    su_telescoping2: "1/(k^2 + 3k + 2)", 0;
    su_mixed: "k + 2^k", 1;
}

#[test]
fn sum_closed_forms_display() {
    let n = Sym::var("n");
    let s = summation(&p("k"), "k", &Sym::one(), &n).unwrap();
    assert_eq!(s.to_string(), "n·(n + 1)/2");
    let s = summation(&p("1/2^k"), "k", &Sym::one(), &infinity()).unwrap();
    assert_eq!(s.to_string(), "1");
    let s = summation(&p("1/(k*(k + 1))"), "k", &Sym::one(), &infinity()).unwrap();
    assert_eq!(s.to_string(), "1");
}

#[test]
fn products() {
    let n = Sym::var("n");
    let r = product(&p("k"), "k", &Sym::one(), &n).unwrap();
    assert_eq!(simplify(&r.subs("n", &Sym::int(5))).to_string(), "120");
    let r = product(&p("2"), "k", &Sym::one(), &n).unwrap();
    assert_eq!(r.to_string(), "2^n");
    let r = product(&p("2^k"), "k", &Sym::one(), &n).unwrap();
    assert_eq!(simplify(&r.subs("n", &Sym::int(3))).to_string(), "64");
}

#[test]
fn calculus_steps() {
    let (_, st) = trace(|| integrate(&p("x*e^x"), "x"));
    assert!(
        st.iter().any(|s| s.rule == "integration by parts"),
        "{st:?}"
    );
    let (_, st) = trace(|| limit(&p("sin(x)/x"), "x", &Point::Finite(Sym::zero()), Dir::Both));
    assert!(st.iter().any(|s| s.rule == "series expansion"), "{st:?}");
    let _ = Lim::PosInf;
}
