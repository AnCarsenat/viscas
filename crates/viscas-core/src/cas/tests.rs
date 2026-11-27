//! Unit tests of the symbolic core (structural parsing, no evaluator).

use super::convert::parse_sym;
use super::*;

fn p(s: &str) -> Sym {
    parse_sym(s).unwrap_or_else(|e| panic!("parse `{s}`: {e}"))
}

fn show(s: &str) -> String {
    p(s).to_string()
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

cases! { show,
    canon_like_terms: "x + x" => "2x";
    canon_poly_order: "1 + 2*x + x^2" => "x^2 + 2x + 1";
    canon_powers: "x * x^2" => "x^3";
    canon_cancel: "x - x" => "0";
    canon_distribute: "2*(x + 1)" => "2x + 2";
    canon_sqrt2_half: "sqrt(2)/2" => "√2/2";
    canon_recip_sqrt: "1/sqrt(2)" => "√2/2";
    canon_sqrt12: "sqrt(12)" => "2√3";
    canon_sqrt_prod: "sqrt(2)*sqrt(3)" => "√6";
    canon_sqrt_sq: "sqrt(2)*sqrt(2)" => "2";
    canon_cbrt: "8^(1/3)" => "2";
    canon_cbrt16: "16^(1/3)" => "2∛2";
    canon_neg_cbrt: "(-8)^(1/3)" => "-2";
    canon_sqrt_neg: "sqrt(-4)" => "2i";
    canon_pi_quarter: "pi/4" => "π/4";
    canon_two_pi: "2*pi" => "2π";
    canon_sin_pi: "sin(pi)" => "0";
    canon_cos_pi: "cos(pi)" => "-1";
    canon_sin_pi4: "sin(pi/4)" => "√2/2";
    canon_cos_pi6: "cos(pi/6)" => "√3/2";
    canon_tan_pi3: "tan(pi/3)" => "√3";
    canon_sin_7pi6: "sin(7pi/6)" => "-1/2";
    canon_cos_neg: "cos(-x)" => "cos(x)";
    canon_sin_neg: "sin(-x)" => "-sin(x)";
    canon_sin_shift: "sin(x + pi)" => "-sin(x)";
    canon_cos_shift: "cos(x + pi/2)" => "-sin(x)";
    canon_asin_half: "asin(1/2)" => "π/6";
    canon_acos_half: "acos(1/2)" => "π/3";
    canon_atan_one: "atan(1)" => "π/4";
    canon_atan_neg: "atan(-sqrt(3))" => "-π/3";
    canon_ln_e: "ln(e)" => "1";
    canon_ln_exp: "ln(e^x)" => "x";
    canon_exp_ln: "e^(ln(x))" => "x";
    canon_exp_ipi: "e^(i*pi)" => "-1";
    canon_ln1: "ln(1)" => "0";
    canon_abs_num: "abs(-3)" => "3";
    canon_abs_pi: "abs(pi - 4)" => "4 - π";
    canon_abs_coeff: "abs(-2x)" => "2|x|";
    canon_rational: "x/2 + x/3" => "5x/6";
    canon_neg_term: "1 - x" => "-x + 1";
    canon_div: "x/(2*y)" => "x/(2y)";
    canon_frac_sum: "1/(x + 1)" => "1/(x + 1)";
    canon_exp_neg: "e^(-x)" => "e^(-x)";
    canon_mul_vars: "x*y*2" => "2x·y";
    canon_power_of_product: "(2*x)^2" => "4x^2";
    canon_nested_pow: "(x^2)^3" => "x^6";
    canon_zero_pow: "0^2" => "0";
    canon_one_pow: "1^x" => "1";
    canon_gamma: "gamma(5)" => "24";
    canon_floor: "floor(7/2)" => "3";
    canon_floor_pi: "floor(pi)" => "3";
    canon_rel: "x^2 == 4" => "x^2 = 4";
    canon_sin_asin: "sin(asin(x))" => "x";
    canon_cos_asin: "cos(asin(x))" => "√(-x^2 + 1)";
    canon_complex: "(1 + i)^2" => "2i";
    canon_sqrt_frac: "sqrt(1/3)" => "√3/3";
    canon_pow_frac: "2^(3/2)" => "2√2";
    canon_sin_pi12: "sin(pi/12)" => "√6/4 - √2/4";
}

fn ex(s: &str) -> String {
    expand(&p(s)).to_string()
}

cases! { ex,
    expand_square: "(x + 1)^2" => "x^2 + 2x + 1";
    expand_diff_squares: "(x - 1)*(x + 1)" => "x^2 - 1";
    expand_cube: "(x + y)^3" => "x^3 + 3x^2·y + 3x·y^2 + y^3";
    expand_nested: "x*(x + 1)*(x + 2)" => "x^3 + 3x^2 + 2x";
    expand_surd: "(1 + sqrt(2))^2" => "2√2 + 3";
    expand_neg_pow: "(x + 1)^(-2)" => "1/(x^2 + 2x + 1)";
    expand_inside_fn: "sin((x + 1)^2)" => "sin(x^2 + 2x + 1)";
}

fn fa(s: &str) -> String {
    factor(&p(s)).to_string()
}

cases! { fa,
    factor_diff_squares: "x^2 - 1" => "(x - 1)(x + 1)";
    factor_square: "x^2 + 2x + 1" => "(x + 1)^2";
    factor_content: "2x^2 + 4x" => "2x·(x + 2)";
    factor_cubic: "x^3 - 6x^2 + 11x - 6" => "(x - 3)(x - 2)(x - 1)";
    factor_rational_root: "2x^2 - x - 1" => "(x - 1)(2x + 1)";
    factor_irreducible: "x^2 + 1" => "x^2 + 1";
    factor_quartic_kron: "x^4 + 4" => "(x^2 - 2x + 2)(x^2 + 2x + 2)";
    factor_x4_1: "x^4 - 1" => "(x - 1)(x + 1)(x^2 + 1)";
    factor_homog: "x^2 - y^2" => "(x - y)(x + y)";
    factor_homog_cube: "x^3 + y^3" => "(x + y)(x^2 - x·y + y^2)";
    factor_rational_fn: "(x^2 - 1)/(x^2 + 2x + 1)" => "(x - 1)/(x + 1)";
    factor_sin: "sin(x)^2 - 1" => "(sin(x) - 1)(sin(x) + 1)";
    factor_neg: "-x^2 + 1" => "-(x - 1)(x + 1)";
    factor_sextic: "x^6 - 1" => "(x - 1)(x + 1)(x^2 - x + 1)(x^2 + x + 1)";
}

fn ca(s: &str) -> String {
    cancel(&p(s)).to_string()
}

cases! { ca,
    cancel_simple: "(x^2 - 1)/(x - 1)" => "x + 1";
    cancel_sum: "1/x + 1/y" => "(x + y)/(x·y)";
    cancel_multi: "(x^2 - y^2)/(x - y)" => "x + y";
    cancel_fracs: "1/(x - 1) - 1/(x + 1)" => "2/(x^2 - 1)";
    cancel_const_den: "(2x + 4)/2" => "x + 2";
}

fn simp(s: &str) -> String {
    simplify(&p(s)).to_string()
}

cases! { simp,
    simplify_pythagoras: "sin(x)^2 + cos(x)^2" => "1";
    simplify_pyth2: "1 - sin(x)^2" => "cos(x)^2";
    simplify_rational: "(x^2 - 1)/(x + 1)" => "x - 1";
    simplify_tan: "sin(x)/cos(x)" => "sin(x)/cos(x)";
    simplify_tan2: "tan(x)*cos(x)" => "sin(x)";
    simplify_cosh: "cosh(x)^2 - sinh(x)^2" => "1";
    simplify_log: "ln(8) - ln(2)" => "2ln(2)";
    simplify_sqrt_sq: "sqrt(x^2)" => "|x|";
    simplify_rationalize: "1/(1 + sqrt(2))" => "√2 - 1";
    simplify_fractions: "x/(x + 1) + 1/(x + 1)" => "1";
}

fn d(s: &str) -> String {
    diff(&p(s), "x").unwrap().to_string()
}

cases! { d,
    diff_power: "x^3" => "3x^2";
    diff_const: "5" => "0";
    diff_linear: "3x + 2" => "3";
    diff_sin: "sin(x)" => "cos(x)";
    diff_cos: "cos(x)" => "-sin(x)";
    diff_tan: "tan(x)" => "tan(x)^2 + 1";
    diff_exp: "e^x" => "e^x";
    diff_exp_chain: "e^(2x)" => "2e^(2x)";
    diff_ln: "ln(x)" => "1/x";
    diff_sqrt: "sqrt(x)" => "1/(2√x)";
    diff_product: "x*sin(x)" => "x·cos(x) + sin(x)";
    diff_chain: "sin(x^2)" => "2x·cos(x^2)";
    diff_quotient: "1/x" => "-1/x^2";
    diff_asin: "asin(x)" => "1/√(-x^2 + 1)";
    diff_atan: "atan(x)" => "1/(x^2 + 1)";
    diff_xx: "x^x" => "x^x·(ln(x) + 1)";
    diff_2x: "2^x" => "2^x·ln(2)";
    diff_sinh: "sinh(x)" => "cosh(x)";
    diff_abs: "abs(x)" => "sign(x)";
    diff_other_var: "y*x^2" => "2x·y";
}

#[test]
fn approx_values() {
    let v = approx_f64(&p("sqrt(2)/2")).unwrap();
    assert!((v - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-15);
    let v = approx_f64(&p("pi + e")).unwrap();
    assert!((v - (std::f64::consts::PI + std::f64::consts::E)).abs() < 1e-14);
}

#[test]
fn partfrac_basic() {
    let r = partfrac(&p("1/(x^2 - 1)"), "x").unwrap();
    assert_eq!(r.to_string(), "1/(2(x - 1)) - 1/(2(x + 1))");
}

#[test]
fn steps_recorded() {
    let (_, steps) = trace(|| diff(&p("sin(x^2)"), "x").unwrap());
    assert!(steps.iter().any(|s| s.rule == "chain rule"), "{steps:?}");
    let (_, none) = trace(|| ());
    assert!(none.is_empty());
}
