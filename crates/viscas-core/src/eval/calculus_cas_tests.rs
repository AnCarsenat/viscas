//! Evaluator-level tests of symbolic calculus: ∫, limit, series, ∑/∏.

use super::*;

fn ev(src: &str) -> String {
    let mut s = Session::new();
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

cases! {
    indefinite_syntax: "∫ u^2 du" => "u^3/3";
    indefinite_trig: "simplify(∫ cos(2x) dx)" => "sin(2x)/2";
    indefinite_parts: "simplify(∫ x sin(x) dx)" => "-x·cos(x) + sin(x)";
    definite_exact: "∫(0, 1) x^2 dx" => "1/3";
    definite_pi: "∫(0, π) sin(x) dx" => "2";
    definite_atan: "∫(0, 1) 1/(1 + x^2) dx" => "π/4";
    definite_infinite: "∫(0, ∞) e^(-x) dx" => "1";
    definite_gauss_numeric: "∫(0, 1) e^(-x^2) dx" => "≈0.746824132812";
    definite_log: "∫(1, e) 1/x dx" => "1";
    integrate_builtin: "integrate(x^3, x)" => "x^4/4";
    integrate_bounds: "integrate(x, x, 0, a)" => "a^2/2";
    integrate_infinite: "integrate(1/x^2, x, 1, ∞)" => "1";
    limit_sinc: "limit(sin(x)/x, x, 0)" => "1";
    limit_inf: "limit((3x^2 + 1)/(x^2 - x), x, ∞)" => "3";
    limit_e: "limit((1 + 1/n)^n, n, ∞)" => "e";
    limit_right: "limit(1/x, x, 0, \"+\")" => "∞";
    limit_left: "limit(1/x, x, 0, \"-\")" => "-∞";
    limit_neg_inf: "limit(e^x, x, -∞)" => "0";
    limit_param: "limit((x^2 - a^2)/(x - a), x, a)" => "2a";
    series_exp: "series(e^x, x, 0, 4)" => "x^3/6 + x^2/2 + x + 1";
    taylor_alias: "taylor(cos(x), x, 0, 5)" => "x^4/24 - x^2/2 + 1";
    series_default: "series(1/(1 - x))" => "x^5 + x^4 + x^3 + x^2 + x + 1";
    sum_symbolic: "∑(k = 1, n) k" => "n·(n + 1)/2";
    sum_squares: "sum(k = 1, n) k^2" => "n·(n + 1)(2n + 1)/6";
    sum_infinite_geometric: "∑(k = 0, ∞) (1/3)^k" => "3/2";
    sum_infinite_telescoping: "∑(k = 1, ∞) 1/(k (k + 1))" => "1";
    sum_finite_still_exact: "∑(k = 1, 4) 1/k" => "25/12";
    summation_builtin: "summation(2^k, k, 0, n)" => "2^(n + 1) - 1";
    product_symbolic: "∏(k = 1, n) 2" => "2^n";
    product_builtin: "product(k, k, 1, 6)" => "720";
}

#[test]
fn integral_numeric_fallback_in_float64() {
    let mut s = Session::new();
    s.opts.float64 = true;
    let v = s.eval("∫(0, 1) x^2 dx").unwrap();
    assert!((v.to_f64() - 1.0 / 3.0).abs() < 1e-12);
}

#[test]
fn indefinite_without_antiderivative_is_error() {
    assert_eq!(err("∫ e^(u^2) du").kind, ErrorKind::Unsupported);
}

#[test]
fn limit_two_sided_mismatch() {
    assert!(err("limit(1/x, x, 0)").message.contains("does not exist"));
}

#[test]
fn infinity_outside_cas_is_error() {
    assert_eq!(err("∞ + 1").kind, ErrorKind::Unsupported);
}

#[test]
fn integral_steps() {
    let v = ev("steps(integrate(x e^x, x))");
    assert!(v.contains("integration by parts"), "{v}");
    let v = ev("steps(limit(sin(x)/x, x, 0))");
    assert!(v.contains("series"), "{v}");
}
