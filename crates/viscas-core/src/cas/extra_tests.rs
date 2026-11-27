//! Property-style tests: derivatives against finite differences,
//! factor/expand round trips, identities and exact tables checked
//! numerically, solutions checked by substitution.

use super::convert::parse_sym;
use super::solve::{Solution, SolveOptions, solve};
use super::*;

fn p(s: &str) -> Sym {
    parse_sym(s).unwrap_or_else(|e| panic!("parse `{s}`: {e}"))
}

/// d/dx f at x0 symbolically vs a central difference.
fn check_diff(src: &str, x0: f64) {
    let f = p(src);
    let d = diff(&f, "x").unwrap();
    let h = 1e-5;
    let num = (eval_f64(&f, "x", x0 + h) - eval_f64(&f, "x", x0 - h)) / (2.0 * h);
    let sym = eval_f64(&d, "x", x0);
    assert!(
        (num - sym).abs() <= 1e-5 * num.abs().max(1.0),
        "d/dx {src} at {x0}: symbolic {sym} ({d}) vs numeric {num}"
    );
}

macro_rules! diff_cases {
    ($($name:ident: $src:expr, $x0:expr;)*) => {
        $(
            #[test]
            fn $name() {
                check_diff($src, $x0);
            }
        )*
    };
}

diff_cases! {
    dn_poly: "3x^4 - 2x^3 + x - 7", 1.3;
    dn_rational: "(x^2 + 1)/(x - 3)", 0.7;
    dn_sqrt: "sqrt(x^2 + 1)", 0.4;
    dn_cbrt: "x^(1/3)", 2.0;
    dn_sin_cos: "sin(x)*cos(2x)", 0.9;
    dn_tan: "tan(x^2)", 0.5;
    dn_asin: "asin(x/2)", 0.6;
    dn_acos: "acos(x^2)", 0.3;
    dn_atan: "atan(1/x)", 1.7;
    dn_sinh: "sinh(2x) + cosh(x)", 0.2;
    dn_tanh: "tanh(x)^2", 0.8;
    dn_asinh: "asinh(x)", 0.5;
    dn_acosh: "acosh(x + 2)", 0.5;
    dn_atanh: "atanh(x/3)", 0.5;
    dn_exp: "e^(-x^2)", 0.6;
    dn_exp2: "2^x * x", 1.1;
    dn_ln: "ln(x^2 + x + 1)", 0.5;
    dn_log10: "ln(x)/ln(10)", 3.0;
    dn_xpowx: "x^x", 1.5;
    dn_powsin: "x^(sin(x))", 1.2;
    dn_abs: "abs(x^3 - 1)", 0.5;
    dn_nested: "sin(cos(tan(x)))", 0.3;
    dn_quotient: "x/(1 + e^x)", 0.4;
    dn_product3: "x*sin(x)*e^x", 0.7;
    dn_sec: "1/cos(x)", 0.4;
    dn_root_frac: "1/sqrt(1 - x^2)", 0.5;
}

/// expand(factor(e)) == expand(e) and the factorization has `n` factors.
fn round_trip(src: &str, n: usize) {
    let e = p(src);
    let f = factor(&e);
    assert_eq!(expand(&f), expand(&e), "{src} → {f}");
    let count = f.factors().iter().filter(|x| !x.is_num()).count();
    assert_eq!(count, n, "{src} → {f}");
}

macro_rules! factor_cases {
    ($($name:ident: $src:expr, $n:expr;)*) => {
        $(
            #[test]
            fn $name() {
                round_trip($src, $n);
            }
        )*
    };
}

factor_cases! {
    ft_quad: "x^2 + 5x + 6", 2;
    ft_cubic: "x^3 + 2x^2 - x - 2", 3;
    ft_lead: "6x^2 + x - 2", 2;
    ft_content: "4x^3 - 4x", 3;
    ft_square: "x^4 - 2x^2 + 1", 2;
    ft_cube: "x^3 + 3x^2 + 3x + 1", 1;
    ft_sophie: "x^4 + 4", 2;
    ft_quartic_quads: "x^4 + x^2 + 1", 2;
    ft_cyclotomic: "x^5 - 1", 2;
    ft_x8: "x^8 - 1", 4;
    ft_irreducible: "x^4 + 1", 1;
    ft_mixed: "(x^2 + 1)*(x - 2)^2*(3x + 1)", 3;
    ft_quintic_split: "(x^2 + x + 1)*(x^3 - 2)", 2;
    ft_sextic_split: "(x^3 + x + 1)*(x^3 - x + 1)", 2;
    ft_bivariate: "x^2 + 2*x*y + y^2", 1;
    ft_bivariate_cube: "x^3 - y^3", 2;
    ft_monomial: "x^3*y + x*y^3", 3;
    ft_rational_coeffs: "x^2/2 - 1/2", 2;
    ft_trig_atom: "sin(x)^2 - 2*sin(x) + 1", 1;
    ft_exp_atom: "e^(2*x) - 1", 2;
}

/// Simplification identities: the result must be exactly `want`.
macro_rules! simplify_cases {
    ($($name:ident: $src:expr => $want:expr;)*) => {
        $(
            #[test]
            fn $name() {
                assert_eq!(simplify(&p($src)).to_string(), $want, "simplify {}", $src);
            }
        )*
    };
}

simplify_cases! {
    si_zero_poly: "(x + 1)^2 - x^2 - 2x - 1" => "0";
    si_cancel: "(x^3 - 1)/(x - 1)" => "x^2 + x + 1";
    si_fracs: "1/(x*(x + 1)) + 1/(x + 1)" => "1/x";
    si_trig_tan: "tan(x)^2 + 1 - 1/cos(x)^2" => "0";
    si_trig_double: "2 - 2*sin(x)^2 - 2*cos(x)^2" => "0";
    si_exp: "e^x*e^(-x)" => "1";
    si_exp_sum: "e^(2x)/e^x" => "e^x";
    si_ln_exp: "ln(e^(x + 1))" => "x + 1";
    si_sqrt_num: "sqrt(18) - sqrt(8)" => "√2";
    si_surd_frac: "(sqrt(3) + 1)*(sqrt(3) - 1)" => "2";
    si_pow: "x^2*x^3/x^4" => "x";
    si_constant_trig: "sin(π/12)^2 + cos(π/12)^2" => "1";
    si_rational_num: "(2/3 + 1/6)*6" => "5";
    si_abs_square: "sqrt(x^4)" => "x^2";
    si_mixed: "(x^2 - y^2)/(x + y)" => "x - y";
}

#[test]
fn exact_trig_table_matches_numeric() {
    for k in -24..=48 {
        let a = Sym::rat(k, 12) * Sym::pi();
        for f in [Func::Sin, Func::Cos] {
            let s = a.apply(f);
            let want = match f {
                Func::Sin => (k as f64 * std::f64::consts::PI / 12.0).sin(),
                _ => (k as f64 * std::f64::consts::PI / 12.0).cos(),
            };
            assert!(
                !matches!(s.node(), Node::Fn(..)),
                "{f:?}({k}π/12) not evaluated: {s}"
            );
            let got = approx_f64(&s).unwrap();
            assert!((got - want).abs() < 1e-14, "{f:?}({k}π/12) = {s}");
        }
    }
}

#[test]
fn exact_tan_and_tenths_match_numeric() {
    for (p_, q_) in [
        (1, 12),
        (1, 6),
        (1, 4),
        (1, 3),
        (5, 12),
        (7, 12),
        (2, 3),
        (3, 4),
        (5, 6),
        (11, 12),
    ] {
        let s = (Sym::rat(p_, q_) * Sym::pi()).apply(Func::Tan);
        let want = (p_ as f64 / q_ as f64 * std::f64::consts::PI).tan();
        assert!(
            (approx_f64(&s).unwrap() - want).abs() < 1e-12,
            "tan({p_}π/{q_}) = {s}"
        );
    }
    for k in 0..20 {
        let s = (Sym::rat(k, 10) * Sym::pi()).apply(Func::Sin);
        let want = (k as f64 * std::f64::consts::PI / 10.0).sin();
        assert!(
            (approx_f64(&s).unwrap() - want).abs() < 1e-14,
            "sin({k}π/10) = {s}"
        );
    }
}

#[test]
fn inverse_trig_round_trip() {
    for k in -2..=2 {
        let a = Sym::rat(k, 6) * Sym::pi();
        let s = a.apply(Func::Sin);
        assert_eq!(s.apply(Func::Asin), a, "asin(sin({a}))");
    }
    let a = Sym::rat(1, 3) * Sym::pi();
    assert_eq!(a.apply(Func::Tan).apply(Func::Atan), a);
}

/// Every solution of a polynomial with integer roots and every other
/// generated equation satisfies it exactly.
macro_rules! solve_cases {
    ($($name:ident: $src:expr, $count:expr;)*) => {
        $(
            #[test]
            fn $name() {
                let eq = p($src);
                let opts = SolveOptions::default();
                let Solution::Values(v) = solve(&eq, "x", &opts).unwrap() else { panic!("{}", $src) };
                assert_eq!(v.len(), $count, "{}: {v:?}", $src);
                let f = match eq.node() {
                    Node::Rel(items, _) => &items[0] - &items[1],
                    _ => eq.clone(),
                };
                for s in &v {
                    let r = simplify(&f.subs("x", s));
                    let val = approx_f64(&r).unwrap_or(f64::NAN);
                    assert!(val.abs() < 1e-9, "{}: x = {s} leaves {r}", $src);
                }
            }
        )*
    };
}

solve_cases! {
    sv_quartic_roots: "(x - 1)*(x + 2)*(x - 3)*(x + 4) == 0", 4;
    sv_rational_roots: "6x^3 - 11x^2 + 6x - 1 == 0", 3;
    sv_quad_rat: "4x^2 - 4x - 1 == 0", 2;
    sv_cubic_surd: "x^3 - 3x == 0", 3;
    sv_biquad_surd: "x^4 - 4x^2 + 1 == 0", 4;
    sv_sextic_cubic: "x^6 - 9x^3 + 8 == 0", 2;
    sv_rational_eq: "x/(x - 1) == 2", 1;
    sv_rational_eq2: "1/(x - 1) + 1/(x + 1) == 1", 2;
    sv_radical: "sqrt(2x + 3) == x", 1;
    sv_exp_eq: "3^(2x) == 27", 1;
    sv_ln_eq: "ln(2x - 1) == 0", 1;
    sv_abs_eq: "abs(2x - 3) == 5", 2;
    sv_power_eq: "(x - 1)^3 == 8", 1;
    sv_exp_sub: "e^(2x) - 5e^x + 6 == 0", 2;
    sv_log_sub: "ln(x)^2 - 3*ln(x) + 2 == 0", 2;
}
