use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::number::Number;
use crate::syntax::{parse_expr, parse_statement};

fn session() -> Session {
    Session::new()
}

/// Evaluates (possibly multi-statement) source and formats the last value.
fn ev(src: &str) -> String {
    let mut s = session();
    match s.eval(src) {
        Ok(v) => v.to_string(),
        Err(e) => panic!("`{src}` failed: {e}"),
    }
}

fn ev_with(s: &mut Session, src: &str) -> String {
    match s.eval(src) {
        Ok(v) => v.to_string(),
        Err(e) => panic!("`{src}` failed: {e}"),
    }
}

fn err(src: &str) -> EvalError {
    match session().eval(src) {
        Ok(v) => panic!("`{src}` should fail, got {v}"),
        Err(e) => e,
    }
}

fn num(src: &str) -> f64 {
    let mut s = session();
    match s.eval(src) {
        Ok(Value::Number(n)) => n.to_f64(),
        other => panic!("`{src}`: expected a number, got {other:?}"),
    }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
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

macro_rules! approx_cases {
    ($($name:ident: $src:expr => $want:expr;)*) => {
        $(
            #[test]
            fn $name() {
                let got = num($src);
                assert!(close(got, $want, 1e-9), "source: {} got {got}, want {}", $src, $want);
            }
        )*
    };
}

macro_rules! error_cases {
    ($($name:ident: $src:expr => $kind:pat,)*) => {
        $(
            #[test]
            fn $name() {
                let e = err($src);
                assert!(matches!(e.kind, $kind), "source: {} got {:?}", $src, e);
            }
        )*
    };
}

// ----- exact arithmetic ---------------------------------------------------

cases! {
    int_add: "1 + 2" => "3";
    rational_sum: "1/3 + 1/6" => "1/2";
    division_exact: "7 / 2" => "7/2";
    decimal_exact: "0.1 + 0.2" => "3/10";
    decimal_equality: "0.1 + 0.2 == 0.3" => "true";
    precedence: "2 + 3 * 4" => "14";
    power_right_assoc: "2^3^2" => "512";
    neg_power: "-2^2" => "-4";
    negative_exponent: "2^-3" => "1/8";
    rational_power: "(2/3)^-2" => "9/4";
    exact_cube_root_power: "8^(1/3)" => "2";
    exact_sqrt_power: "4^(3/2)" => "8";
    big_integer: "2^100" => "1267650600228229401496703205376";
    zero_pow_zero: "0^0" => "1";
    mod_operator: "7 mod 3" => "1";
    mod_negative: "-7 mod 3" => "2";
    mod_rational: "7/2 mod 1" => "1/2";
    factorial_postfix: "5!" => "120";
    factorial_big: "20!" => "2432902008176640000";
    percent: "50%" => "1/2";
    percent_of: "20% * 50" => "10";
    sqrt_exact: "sqrt(16)" => "4";
    sqrt_rational: "√(9/4)" => "3/2";
    abs_bars: "|-3|" => "3";
    nested_abs: "||-3| - 5|" => "2";
    unary_plus: "+5" => "5";
    sci_literal: "1.5e3" => "1500";
    sci_small: "2e-3" => "1/500";
    sum_of_many: "1+2+3+4+5+6+7+8+9+10" => "55";
    mixed_signs: "1 - 2 + 3 - 4" => "-2";
    complex_literal: "3 + 2i" => "3 + 2i";
    complex_mul: "(1 + i)^2" => "2i";
    complex_i_squared: "i^2" => "-1";
    complex_div: "1 / i" => "-i";
}

approx_cases! {
    pi_value: "π" => std::f64::consts::PI;
    pi_ascii: "pi" => std::f64::consts::PI;
    tau_value: "τ" => std::f64::consts::TAU;
    e_value: "e" => std::f64::consts::E;
    phi_value: "φ" => 1.618_033_988_749_895;
    sqrt2: "√2" => std::f64::consts::SQRT_2;
    irrational_power: "2^(1/2)" => std::f64::consts::SQRT_2;
    sin_value: "sin(1)" => 0.841_470_984_807_896_5;
    cos_pi: "cos(π)" => -1.0;
    exp1: "exp(1)" => std::f64::consts::E;
    ln_e: "ln(e)" => 1.0;
    log10_value: "log(1000)" => 3.0;
    log_base: "log(8, 2)" => 3.0;
    log_subscript: "log_2(8)" => 3.0;
    log_subscript_prefix: "log_2 32" => 5.0;
    lg_alias: "lg 100" => 2.0;
    gamma_half: "gamma(0.5)" => 1.772_453_850_905_516;
    factorial_half: "0.5!" => 0.886_226_925_452_758;
    atan2_value: "atan2(1, 1)" => std::f64::consts::FRAC_PI_4;
    acot_zero: "acot(0)" => std::f64::consts::FRAC_PI_2;
    sinh_value: "sinh(1)" => 1.175_201_193_643_801_4;
    asinh_alias: "arsinh(1)" => 0.881_373_587_019_543;
    sec_value: "sec(0)" => 1.0;
    cot_value: "cot(1)" => 0.642_092_615_934_330_7;
    real_cbrt: "cbrt(-27)" => -3.0;
    root_odd_negative: "root(-32, 5)" => -2.0;
}

#[test]
fn sin_squared_identity_via_let() {
    let v = num("let x = 0.7\nsin(x)^2 + cos(x)^2");
    assert!(close(v, 1.0, 1e-12));
}

// ----- implicit multiplication, split names, call vs multiply ------------

#[test]
fn implicit_multiplication() {
    let mut s = session();
    ev_with(&mut s, "let x = 3");
    assert_eq!(ev_with(&mut s, "2x"), "6");
    assert_eq!(ev_with(&mut s, "2(x + 1)"), "8");
    assert_eq!(ev_with(&mut s, "1/2x"), "1/6");
    assert_eq!(ev_with(&mut s, "3x^2"), "27");
}

#[test]
fn implicit_mult_with_space_and_number() {
    let mut s = session();
    ev_with(&mut s, "a := 4");
    assert_eq!(ev_with(&mut s, "a 2"), "8");
    assert_eq!(ev_with(&mut s, "(a)(a)"), "16");
    assert_eq!(ev_with(&mut s, "-2a"), "-8");
}

#[test]
fn split_identifier_product() {
    let mut s = session();
    ev_with(&mut s, "let x = 2");
    ev_with(&mut s, "let y = 5");
    assert_eq!(ev_with(&mut s, "xy"), "10");
    assert_eq!(ev_with(&mut s, "3xy"), "30");
    assert_eq!(ev_with(&mut s, "xyx"), "20");
}

#[test]
fn split_prefers_defined_multi_letter_names() {
    let mut s = session();
    ev_with(&mut s, "ab := 10");
    ev_with(&mut s, "c := 3");
    assert_eq!(ev_with(&mut s, "abc"), "30");
}

#[test]
fn split_with_pi() {
    let mut s = session();
    ev_with(&mut s, "let r = 2");
    let v = s.eval("2pir").unwrap();
    assert!(close(v.to_f64(), 4.0 * std::f64::consts::PI, 1e-12));
}

#[test]
fn split_with_digits_and_subscript() {
    let mut s = session();
    ev_with(&mut s, "a := 2");
    ev_with(&mut s, "b2 := 7");
    assert_eq!(ev_with(&mut s, "ab2"), "14");
    ev_with(&mut s, "y_1 := 5");
    assert_eq!(ev_with(&mut s, "ay_1"), "10");
}

#[test]
fn split_function_piece_applies() {
    let mut s = session();
    ev_with(&mut s, "let x = 0");
    assert_eq!(ev_with(&mut s, "cosx"), "1");
}

#[test]
fn split_unknown_name_error_with_span() {
    let mut s = session();
    ev_with(&mut s, "let x = 2");
    let e = s.eval("1 + xq").unwrap_err();
    assert_eq!(e.kind, ErrorKind::UnknownName("xq".into()));
    assert_eq!(e.message, "unknown name `xq`");
    assert_eq!((e.span.start, e.span.end), (4, 6));
}

#[test]
fn defined_name_is_not_split() {
    let mut s = session();
    ev_with(&mut s, "let x = 2");
    ev_with(&mut s, "let y = 3");
    ev_with(&mut s, "xy := 100");
    assert_eq!(ev_with(&mut s, "xy"), "100");
}

#[test]
fn call_vs_multiply_number() {
    let mut s = session();
    ev_with(&mut s, "let x = 3");
    ev_with(&mut s, "let y = 4");
    assert_eq!(ev_with(&mut s, "x(y + 1)"), "15");
}

#[test]
fn call_vs_multiply_function() {
    let mut s = session();
    ev_with(&mut s, "f(x) := x^2");
    assert_eq!(ev_with(&mut s, "f(3)"), "9");
    assert_eq!(ev_with(&mut s, "2f(3)"), "18");
}

#[test]
fn call_vs_multiply_tuple() {
    let mut s = session();
    ev_with(&mut s, "k := 2");
    assert_eq!(ev_with(&mut s, "k(1, 2)"), "(2, 4)");
}

#[test]
fn split_call_applies_last_piece() {
    let mut s = session();
    ev_with(&mut s, "a := 3");
    ev_with(&mut s, "f(x) := x + 1");
    assert_eq!(ev_with(&mut s, "af(1)"), "6");
}

#[test]
fn prefix_function_application() {
    assert!(close(num("sin 2 * 1"), (2.0f64).sin(), 1e-12));
    let mut s = session();
    ev_with(&mut s, "let x = 0.5");
    let v = s.eval("sin 2x").unwrap().to_f64();
    assert!(close(v, 1.0f64.sin(), 1e-12));
    let v = s.eval("sin x cos x").unwrap().to_f64();
    assert!(close(v, 0.5f64.sin() * 0.5f64.cos(), 1e-12));
}

#[test]
fn ascii_alias_theta() {
    let mut s = session();
    ev_with(&mut s, "let theta = 2");
    assert_eq!(ev_with(&mut s, "θ + 1"), "3");
}

// ----- errors ------------------------------------------------------------

error_cases! {
    division_by_zero: "1/0" => ErrorKind::Number(_),
    unknown_name: "foo + 1" => ErrorKind::UnknownName(_),
    sqrt_negative_real_mode: "sqrt(-1)" => ErrorKind::NotReal,
    ln_negative_real_mode: "ln(-1)" => ErrorKind::NotReal,
    even_root_negative: "(-8)^(1/2)" => ErrorKind::NotReal,
    asin_domain: "asin(2)" => ErrorKind::Number(_),
    list_plus_bool: "[1, 2] + true" => ErrorKind::Type,
    call_arity: "sin(1, 2)" => ErrorKind::Arity,
    index_out_of_range: "[1, 2, 3][4]" => ErrorKind::Index,
    index_zero: "[1, 2, 3][0]" => ErrorKind::Index,
    index_fraction: "[1, 2, 3][1.5]" => ErrorKind::Index,
    units_unsupported: "5 km to m" => ErrorKind::Unsupported,
    indefinite_integral: "∫ 2 dx" => ErrorKind::Unsupported,
    infinite_sum: "∑(k = 1, ∞) 1/k^2" => ErrorKind::Unsupported,
    redefine_builtin: "sin := 3" => ErrorKind::Definition,
    redefine_constant: "π := 3" => ErrorKind::Definition,
    complex_ordering: "i < 1" => ErrorKind::NotReal,
    list_length_mismatch: "[1, 2] + [1, 2, 3]" => ErrorKind::Other,
    matrix_size_mismatch: "[[1, 2], [3, 4]] + [[1, 2, 3], [4, 5, 6]]" => ErrorKind::Other,
    point_times_point: "(1, 2) * (3, 4)" => ErrorKind::Type,
    bare_prime: "sin'" => ErrorKind::Type,
    regression_unsupported: "y ~ 2x" => ErrorKind::Unsupported,
    parse_error: "1 +" => ErrorKind::Parse,
    range_wrong_way: "[1, 0..5]" => ErrorKind::Other,
    not_a_condition: "{1: 2, 3}" => ErrorKind::Type,
    infinity_outside_bound: "∞ + 1" => ErrorKind::Unsupported,
    member_missing: "(1, 2).q" => ErrorKind::Type,
}

#[test]
fn errors_carry_spans() {
    let e = err("2 + 1/0");
    assert_eq!((e.span.start, e.span.end), (6, 7));
    let e = err("3 * [1,2][5]");
    assert_eq!(e.kind, ErrorKind::Index);
    assert_eq!((e.span.start, e.span.end), (10, 11));
}

// ----- comparisons and logic ----------------------------------------------

cases! {
    compare_lt: "1 < 2" => "true";
    compare_chain_true: "0 < 1 < 2" => "true";
    compare_chain_false: "0 < 3 < 2" => "false";
    compare_eq_kinds: "1 == 1.0" => "true";
    compare_ne: "1 != 2" => "true";
    compare_ge_unicode: "2 ≥ 2" => "true";
    logic_and: "1 < 2 and 2 < 3" => "true";
    logic_or: "1 > 2 or 2 < 3" => "true";
    logic_not: "not 1 > 2" => "true";
    string_eq: "\"a\" == \"a\"" => "true";
    point_eq: "(1, 2) == (1, 2)" => "true";
    list_compare_broadcast: "[1, 2, 3] > 1" => "[false, true, true]";
    list_and_broadcast: "[1, 2, 3] > 1 and [1, 2, 3] < 3" => "[false, true, false]";
}

// ----- piecewise and restrictions ----------------------------------------

cases! {
    piecewise_first: "{1 < 2: 10, 20}" => "10";
    piecewise_otherwise: "{1 > 2: 10, 20}" => "20";
    piecewise_no_match: "{1 > 2: 10}" => "undefined";
    piecewise_condition_only: "{2 > 1}" => "1";
    piecewise_chain: "{1 > 2: 1, 2 > 1: 2, 3}" => "2";
    piecewise_list: "{[-1, 2] < 0: 0, 1}" => "[0, 1]";
    restriction_holds: "(5 {1 < 2})" => "5";
    restriction_fails: "(5 {1 > 2})" => "undefined";
    statement_restriction: "5 {1 > 2}" => "undefined";
    undefined_propagates: "{1 > 2: 1} + 1" => "undefined";
    restriction_list: "([1, 2, 3] {[1, 2, 3] > 1})" => "[undefined, 2, 3]";
}

#[test]
fn piecewise_abs_function() {
    let mut s = session();
    ev_with(&mut s, "g(x) := {x < 0: -x, x}");
    assert_eq!(ev_with(&mut s, "g(-3)"), "3");
    assert_eq!(ev_with(&mut s, "g(4)"), "4");
    assert_eq!(ev_with(&mut s, "g([-1, 2])"), "[1, 2]");
}

#[test]
fn restricted_function_definition() {
    let mut s = session();
    ev_with(&mut s, "h(x) := x^2 {0 < x < 2}");
    assert_eq!(ev_with(&mut s, "h(1)"), "1");
    assert_eq!(ev_with(&mut s, "h(3)"), "undefined");
}

// ----- lists, ranges, comprehensions, indexing ---------------------------

cases! {
    list_literal: "[1, 2, 3]" => "[1, 2, 3]";
    empty_list: "[]" => "[]";
    list_broadcast_add: "[1, 2, 3] + 1" => "[2, 3, 4]";
    list_broadcast_mul: "2 * [1, 2, 3]" => "[2, 4, 6]";
    list_elementwise: "[1, 2] * [3, 4]" => "[3, 8]";
    list_power: "[1, 2, 3]^2" => "[1, 4, 9]";
    list_function_broadcast: "sqrt([4, 9])" => "[2, 3]";
    list_neg: "-[1, 2]" => "[-1, -2]";
    range_simple: "[1..5]" => "[1, 2, 3, 4, 5]";
    range_ellipsis: "[1...3]" => "[1, 2, 3]";
    range_down: "[3..1]" => "[3, 2, 1]";
    range_step: "[0, 2..10]" => "[0, 2, 4, 6, 8, 10]";
    range_exact_decimal: "len([0, 0.1..1])" => "11";
    range_exact_values: "[0, 0.25..1]" => "[0, 1/4, 1/2, 3/4, 1]";
    range_not_reaching_end: "[1, 3..6]" => "[1, 3, 5]";
    range_builtin: "range(1, 2, 1/2)" => "[1, 3/2, 2]";
    comprehension_squares: "[k^2 for k = [1..5]]" => "[1, 4, 9, 16, 25]";
    comprehension_in: "[2k for k in [1, 2]]" => "[2, 4]";
    comprehension_two: "[(i, j) for i = [1, 2], j = [3, 4]]" => "[(1, 3), (1, 4), (2, 3), (2, 4)]";
    comprehension_dependent: "[j for i = [1..3], j = [1..i]]" => "[1, 1, 2, 1, 2, 3]";
    index_first: "[10, 20, 30][1]" => "10";
    index_last: "[10, 20, 30][3]" => "30";
    index_list: "[10, 20, 30][[1, 3]]" => "[10, 30]";
    index_range: "[10, 20, 30, 40][2..3]" => "[20, 30]";
    index_filter: "[1..6][[1..6] > 3]" => "[4, 5, 6]";
    index_matrix_element: "[[1, 2], [3, 4]][2, 1]" => "3";
    index_matrix_row: "[[1, 2], [3, 4]][2]" => "[3, 4]";
    index_tuple: "(5, 6)[2]" => "6";
    index_string: "\"abc\"[2]" => "\"b\"";
    nested_list: "[[1], [2, 3]]" => "[[1], [2, 3]]";
    list_of_points_shift: "[(1, 2), (3, 4)] + (1, 1)" => "[(2, 3), (4, 5)]";
}

#[test]
fn list_filter_by_variable() {
    let mut s = session();
    ev_with(&mut s, "L := [5, 1, 4, 2]");
    assert_eq!(ev_with(&mut s, "L[L > 3]"), "[5, 4]");
    assert_eq!(ev_with(&mut s, "L[2]"), "1");
    assert_eq!(ev_with(&mut s, "len(L)"), "4");
}

// ----- points, members, matrices ------------------------------------------

cases! {
    point_add: "(1, 2) + (3, 4)" => "(4, 6)";
    point_sub: "(1, 2, 3) - (1, 1, 1)" => "(0, 1, 2)";
    point_scale: "2(1, 2)" => "(2, 4)";
    point_scale_right: "(1, 2) * 3" => "(3, 6)";
    point_divide: "(2, 4) / 2" => "(1, 2)";
    member_x: "(3, 4).x" => "3";
    member_y: "(3, 4).y" => "4";
    member_z: "(3, 4, 5).z" => "5";
    member_list_map: "[(1, 2), (3, 4)].x" => "[1, 3]";
    member_re_im: "(3 + 4i).im" => "4";
    method_call_sort: "[3, 1, 2].sort()" => "[1, 2, 3]";
    method_call_sum: "[1, 2, 3].sum()" => "6";
    matrix_literal: "[[1, 2], [3, 4]]" => "[[1, 2], [3, 4]]";
    matrix_add: "[[1, 2], [3, 4]] + [[1, 1], [1, 1]]" => "[[2, 3], [4, 5]]";
    matrix_sub: "[[1, 2], [3, 4]] - [[1, 2], [3, 4]]" => "[[0, 0], [0, 0]]";
    matrix_mul: "[[1, 2], [3, 4]] * [[5, 6], [7, 8]]" => "[[19, 22], [43, 50]]";
    matrix_scalar: "2 * [[1, 2], [3, 4]]" => "[[2, 4], [6, 8]]";
    matrix_vector: "[[1, 2], [3, 4]] * [1, 1]" => "[3, 7]";
    matrix_power: "[[1, 1], [1, 0]]^10" => "[[89, 55], [55, 34]]";
    matrix_transpose: "transpose([[1, 2, 3], [4, 5, 6]])" => "[[1, 4], [2, 5], [3, 6]]";
    matrix_member_t: "[[1, 2], [3, 4]].T" => "[[1, 3], [2, 4]]";
    matrix_rect_mul: "[[1, 2, 3]] * [[1], [2], [3]]" => "[[14]]";
}

#[test]
fn point_members_of_variable() {
    let mut s = session();
    ev_with(&mut s, "P := (1, 2)");
    ev_with(&mut s, "Q := (4, 6)");
    assert_eq!(ev_with(&mut s, "Q - P"), "(3, 4)");
    assert_eq!(ev_with(&mut s, "sqrt((Q.x - P.x)^2 + (Q.y - P.y)^2)"), "5");
}

// ----- builtins --------------------------------------------------------------

cases! {
    b_abs_neg: "abs(-5/2)" => "5/2";
    b_abs_complex: "abs(3 + 4i)" => "5";
    b_sign: "sign(-3)" => "-1";
    b_floor: "floor(7/2)" => "3";
    b_floor_neg: "floor(-7/2)" => "-4";
    b_ceil: "ceil(7/2)" => "4";
    b_round: "round(5/2)" => "3";
    b_round_digits: "round(1.2345, 2)" => "123/100";
    b_trunc: "trunc(-7/2)" => "-3";
    b_frac: "frac(7/2)" => "1/2";
    b_frac_neg: "frac(-1/4)" => "3/4";
    b_mod: "10 mod 4" => "2";
    b_gcd: "gcd(12, 18)" => "6";
    b_gcd_variadic: "gcd(12, 18, 8)" => "2";
    b_gcd_list: "gcd([12, 18])" => "6";
    b_lcm: "lcm(4, 6)" => "12";
    b_fact: "fact(6)" => "720";
    b_gamma_int: "gamma(5)" => "24";
    b_binom: "binom(5, 2)" => "10";
    b_perm: "perm(5, 2)" => "20";
    b_min_variadic: "min(3, 1, 2)" => "1";
    b_max_list: "max([3, 7, 2])" => "7";
    b_min_rational: "min(1/2, 1/3)" => "1/3";
    b_sum: "sum([1, 2, 3, 4])" => "10";
    b_sum_empty: "sum([])" => "0";
    b_prod: "prod([1, 2, 3, 4])" => "24";
    b_prod_empty: "prod([])" => "1";
    b_sum_points: "sum([(1, 2), (3, 4)])" => "(4, 6)";
    b_len: "len([1, 2, 3])" => "3";
    b_len_string: "len(\"hello\")" => "5";
    b_sort: "sort([3, 1/2, -1])" => "[-1, 1/2, 3]";
    b_sort_strings: "sort([\"b\", \"a\"])" => "[\"a\", \"b\"]";
    b_reverse: "reverse([1, 2, 3])" => "[3, 2, 1]";
    b_range: "range(1, 4)" => "[1, 2, 3, 4]";
    b_isprime_true: "isprime(97)" => "true";
    b_isprime_false: "isprime(91)" => "false";
    b_isprime_list: "isprime([2, 4])" => "[true, false]";
    b_factor: "factor(360)" => "[2, 2, 2, 3, 3, 5]";
    b_factor_negative: "factor(-12)" => "[-1, 2, 2, 3]";
    b_exact: "exact(0.75)" => "3/4";
    b_exact_float: "exact(num(1/3))" => "1/3";
    b_re: "re(3 + 4i)" => "3";
    b_im: "im(3 + 4i)" => "4";
    b_conj: "conj(3 + 4i)" => "3 - 4i";
    b_sqrt_list: "sqrt([1, 4, 9])" => "[1, 2, 3]";
    b_cbrt_exact: "cbrt(27)" => "3";
    b_cbrt_symbol: "∛(-8)" => "-2";
    b_root_exact: "root(16, 4)" => "2";
    b_exp_zero: "exp(0)" => "1";
    b_ln_one: "ln(1)" => "0";
    b_sin_zero: "sin(0)" => "0";
    b_string_concat: "\"ab\" + \"cd\"" => "\"abcd\"";
}

#[test]
fn num_forces_decimal() {
    let v = session().eval("num(1/3)").unwrap();
    let Value::Number(n) = &v else { panic!() };
    assert!(!n.is_exact());
    assert_eq!(format_value(&v, &FormatOptions::approx(5)), "0.33333");
}

#[test]
fn builtin_alias_arcsin() {
    assert!(close(num("arcsin(1)"), std::f64::consts::FRAC_PI_2, 1e-12));
    assert!(close(num("sin^-1(1)"), std::f64::consts::FRAC_PI_2, 1e-12));
}

#[test]
fn builtin_table_documented_names_resolve() {
    for b in BUILTINS {
        assert!(is_reserved(b.name), "{}", b.name);
    }
    for (alias, _) in ALIASES {
        assert!(is_reserved(alias), "{alias}");
    }
    assert!(BUILTINS.len() >= 55);
}

// ----- odd roots, complex mode, degree mode -------------------------------

cases! {
    odd_root_real_mode: "(-8)^(1/3)" => "-2";
    odd_root_squared: "(-8)^(2/3)" => "4";
    odd_root_negative_exp: "(-8)^(-1/3)" => "-1/2";
}

#[test]
fn complex_mode_sqrt_negative() {
    let mut s = session();
    s.opts.complex = true;
    assert_eq!(ev_with(&mut s, "sqrt(-4)"), "2i");
    let v = s.eval("ln(-1)").unwrap();
    let Value::Number(n) = v else { panic!() };
    assert!(close(n.im().to_f64(), std::f64::consts::PI, 1e-12));
}

#[test]
fn complex_mode_principal_cube_root() {
    let mut s = session();
    s.opts.complex = true;
    let v = s.eval("(-8)^(1/3)").unwrap();
    let Value::Number(n) = v else { panic!() };
    assert!(close(n.re().to_f64(), 1.0, 1e-12));
    assert!(close(n.im().to_f64(), 3f64.sqrt(), 1e-12));
}

#[test]
fn explicit_i_allowed_in_real_mode() {
    assert_eq!(ev("sqrt(2i)"), ev("sqrt(2i)"));
    assert_eq!(ev("(2i)^2"), "-4");
    assert_eq!(ev("arg(i) == π/2"), "true");
}

#[test]
fn degree_mode_exact_values() {
    let mut s = session();
    s.opts.angle = AngleUnit::Degrees;
    assert_eq!(ev_with(&mut s, "sin(30)"), "1/2");
    assert_eq!(ev_with(&mut s, "cos(60)"), "1/2");
    assert_eq!(ev_with(&mut s, "sin(90)"), "1");
    assert_eq!(ev_with(&mut s, "cos(180)"), "-1");
    assert_eq!(ev_with(&mut s, "tan(45)"), "1");
    assert_eq!(ev_with(&mut s, "tan(135)"), "-1");
    assert_eq!(ev_with(&mut s, "sec(60)"), "2");
    assert_eq!(ev_with(&mut s, "sin(-30)"), "-1/2");
    assert_eq!(ev_with(&mut s, "sin(390)"), "1/2");
}

#[test]
fn degree_mode_numeric_and_inverse() {
    let mut s = session();
    s.opts.angle = AngleUnit::Degrees;
    let v = s.eval("sin(10)").unwrap().to_f64();
    assert!(close(v, 10f64.to_radians().sin(), 1e-12));
    let v = s.eval("asin(1/2)").unwrap().to_f64();
    assert!(close(v, 30.0, 1e-12));
    let v = s.eval("atan2(1, 1)").unwrap().to_f64();
    assert!(close(v, 45.0, 1e-12));
    assert!(s.eval("tan(90)").is_err());
}

#[test]
fn radian_mode_is_default() {
    assert!(close(num("sin(30)"), 30f64.sin(), 1e-12));
}

// ----- user functions, closures, scripting ---------------------------------

#[test]
fn function_definition_and_call() {
    let mut s = session();
    ev_with(&mut s, "f(x) := x^2 + 1");
    assert_eq!(ev_with(&mut s, "f(2)"), "5");
    assert_eq!(ev_with(&mut s, "f(1/2)"), "5/4");
    assert_eq!(ev_with(&mut s, "f([1, 2])"), "[2, 5]");
}

#[test]
fn multi_arg_function() {
    let mut s = session();
    ev_with(&mut s, "g(x, y) := x^2 + y^2");
    assert_eq!(ev_with(&mut s, "g(3, 4)"), "25");
    let e = s.eval("g(1)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Arity);
}

#[test]
fn functions_see_global_redefinitions() {
    let mut s = session();
    ev_with(&mut s, "a := 2");
    ev_with(&mut s, "f(x) := a x");
    assert_eq!(ev_with(&mut s, "f(5)"), "10");
    ev_with(&mut s, "a := 3");
    assert_eq!(ev_with(&mut s, "f(5)"), "15");
}

#[test]
fn function_composition() {
    let mut s = session();
    ev_with(&mut s, "f(x) := x + 1");
    ev_with(&mut s, "g(x) := 2x");
    assert_eq!(ev_with(&mut s, "f(g(3))"), "7");
    assert_eq!(ev_with(&mut s, "g(f(3))"), "8");
}

#[test]
fn parameter_shadows_constant() {
    let mut s = session();
    ev_with(&mut s, "f(e) := e + 1");
    assert_eq!(ev_with(&mut s, "f(1)"), "2");
}

#[test]
fn function_value_display() {
    let mut s = session();
    let v = s.eval("f(x) := x^2").unwrap();
    assert_eq!(v.to_string(), "f(x) = x^2");
    assert_eq!(ev("sin").to_string(), "builtin sin");
}

#[test]
fn recursion_via_fn() {
    let src = "fn fac(n) { if n <= 1 { 1 } else { n * fac(n - 1) } }\nfac(20)";
    assert_eq!(ev(src), "2432902008176640000");
}

#[test]
fn recursion_definition_then_call() {
    let mut s = session();
    ev_with(
        &mut s,
        "fn fib(n) { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }",
    );
    assert_eq!(ev_with(&mut s, "fib(15)"), "610");
}

#[test]
fn let_and_assignment() {
    assert_eq!(ev("let a = 1\na = a + 2\na"), "3");
}

#[test]
fn for_loop_accumulates() {
    assert_eq!(ev("let s = 0\nfor k in [1..10] { s = s + k }\ns"), "55");
}

#[test]
fn while_loop() {
    assert_eq!(ev("let n = 0\nwhile n < 5 { n = n + 1 }\nn"), "5");
}

#[test]
fn if_else_chain() {
    let src =
        "fn cls(x) { if x < 0 { -1 } else if x == 0 { 0 } else { 1 } }\n[cls(-5), cls(0), cls(3)]";
    assert_eq!(ev(src), "[-1, 0, 1]");
}

#[test]
fn early_return() {
    let src = "fn first_big(L) { for x in L { if x > 10 { return x } }\n return -1 }\n[first_big([1, 20, 30]), first_big([1])]";
    assert_eq!(ev(src), "[20, -1]");
}

#[test]
fn block_value_is_last_expression() {
    assert_eq!(ev("fn f(x) { let y = x + 1\n y * 2 }\nf(3)"), "8");
}

#[test]
fn closure_captures_local_by_value() {
    let src = "let k = 10\nfn addk(x) { x + k }\nk = 20\naddk(1)";
    assert_eq!(ev(src), "11");
}

#[test]
fn nested_fn_closure() {
    let src = "fn outer(a) { fn inner(b) { a + b }\n inner(5) }\nouter(1)";
    assert_eq!(ev(src), "6");
}

#[test]
fn let_is_local_to_block() {
    let mut s = session();
    let e = s.eval("if 1 < 2 { let z = 5 }\nz").unwrap_err();
    assert!(matches!(e.kind, ErrorKind::UnknownName(_)));
}

#[test]
fn script_assignment_defines_global() {
    let mut s = session();
    ev_with(&mut s, "let t = 0\nfor k in [1..3] { total := k }\nt");
    assert_eq!(ev_with(&mut s, "total"), "3");
}

#[test]
fn action_updates_simultaneously() {
    let mut s = session();
    ev_with(&mut s, "a := 1");
    ev_with(&mut s, "b := 2");
    ev_with(&mut s, "a -> b, b -> a");
    assert_eq!(ev_with(&mut s, "(a, b)"), "(2, 1)");
}

#[test]
fn run_program_returns_last_value() {
    let prog = crate::syntax::parse_program("let x = 2\nlet y = 3\nx * y").unwrap();
    let mut env = Env::new();
    let v = run_program(&prog, &mut env, &EvalOptions::default()).unwrap();
    assert_eq!(v.to_string(), "6");
    assert!(!env.contains("x"));
}

// ----- budgets and depth ------------------------------------------------------

#[test]
fn infinite_loop_hits_step_budget() {
    let mut s = session();
    s.opts.max_steps = 100_000;
    let e = s.eval("let n = 0\nwhile true { n = n + 1 }").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepLimit);
}

#[test]
fn infinite_recursion_hits_depth_limit() {
    let mut s = session();
    ev_with(&mut s, "fn down(n) { down(n + 1) }");
    let e = s.eval("down(0)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::DepthLimit);
}

#[test]
fn deep_expression_recursion_is_error_not_crash() {
    let mut s = session();
    ev_with(&mut s, "rec(n) := {n <= 0: 0, 1 + rec(n - 1)}");
    assert_eq!(ev_with(&mut s, "rec(20)"), "20");
    let e = s.eval("rec(100000)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::DepthLimit);
}

#[test]
fn deep_recursion_safe_on_small_stack() {
    // Default depth limit must not overflow a 2 MiB stack in debug builds.
    let h = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let mut s = session();
            s.eval("fn deep(n) { if n <= 0 { 0 } else { 1 + deep(n - 1) } }")
                .unwrap();
            s.eval("deep(1000000)").unwrap_err().kind
        })
        .unwrap();
    assert_eq!(h.join().unwrap(), ErrorKind::DepthLimit);
}

#[test]
fn moderate_recursion_succeeds() {
    let mut s = session();
    ev_with(
        &mut s,
        "fn cnt(n) { if n <= 0 { 0 } else { 1 + cnt(n - 1) } }",
    );
    assert_eq!(ev_with(&mut s, "cnt(50)"), "50");
}

#[test]
fn interrupt_flag_cancels() {
    let flag = Arc::new(AtomicBool::new(true));
    let mut s = session();
    s.opts.interrupt = Some(flag);
    let e = s.eval("let n = 0\nwhile true { n = n + 1 }").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Interrupted);
}

#[test]
fn huge_range_is_refused() {
    let e = err("[1..100000000]");
    assert_eq!(e.kind, ErrorKind::Other);
}

// ----- calculus ---------------------------------------------------------------

cases! {
    sum_exact: "∑(k = 1, 100) k" => "5050";
    sum_ascii: "sum(k = 1, 10) k^2" => "385";
    sum_rational: "∑(k = 1, 4) 1/k" => "25/12";
    sum_empty: "∑(k = 5, 1) k" => "0";
    product_exact: "∏(k = 1, 10) k" => "3628800";
    product_ascii: "prod(k = 1, 5) 2" => "32";
    sum_body_extends_over_product: "∑(k = 1, 3) 2k + 1" => "13";
}

#[test]
fn derivative_at_point() {
    let mut s = session();
    ev_with(&mut s, "let x = 2");
    let v = s.eval("d/dx x^3").unwrap().to_f64();
    assert!(close(v, 12.0, 1e-8), "{v}");
    let v = s.eval("d^2/dx^2 x^3").unwrap().to_f64();
    assert!(close(v, 12.0, 1e-5), "{v}");
}

#[test]
fn derivative_without_value_is_unsupported() {
    let e = parse_expr("d/dx x^2").unwrap();
    let r = eval_expr(&e, &Env::new(), &EvalOptions::default()).unwrap_err();
    assert_eq!(r.kind, ErrorKind::Unsupported);
    // At top level it is a plottable relation in x.
    assert!(err("d/dx x^2").message.contains("relation"));
}

#[test]
fn prime_derivative_user_function() {
    let mut s = session();
    ev_with(&mut s, "f(x) := sin(x)");
    let v = s.eval("f'(1)").unwrap().to_f64();
    assert!(close(v, 1f64.cos(), 1e-9), "{v}");
    let v = s.eval("f''(1)").unwrap().to_f64();
    assert!(close(v, -(1f64.sin()), 1e-6), "{v}");
    let v = s.eval("sin'(0)").unwrap().to_f64();
    assert!(close(v, 1.0, 1e-9));
}

#[test]
fn prime_third_order() {
    let mut s = session();
    ev_with(&mut s, "p(x) := x^4");
    let v = s.eval("p'''(1)").unwrap().to_f64();
    assert!(close(v, 24.0, 1e-3), "{v}");
}

#[test]
fn definite_integral_polynomial() {
    let v = num("∫(0, 1) x^2 dx");
    assert!(close(v, 1.0 / 3.0, 1e-12), "{v}");
}

#[test]
fn definite_integral_sin() {
    let v = num("∫(0, π) sin(x) dx");
    assert!(close(v, 2.0, 1e-10), "{v}");
}

#[test]
fn integral_reversed_bounds() {
    let v = num("∫(1, 0) x dx");
    assert!(close(v, -0.5, 1e-12));
}

#[test]
fn integral_infinite_bounds() {
    let v = num("∫(-∞, ∞) exp(-x^2) dx");
    assert!(close(v, std::f64::consts::PI.sqrt(), 1e-8), "{v}");
    let v = num("∫(0, ∞) exp(-x) dx");
    assert!(close(v, 1.0, 1e-8), "{v}");
}

#[test]
fn integral_ascii_and_user_function() {
    let mut s = session();
    ev_with(&mut s, "f(t) := 2t");
    let v = s.eval("int(0, 3) f(t) dt").unwrap().to_f64();
    assert!(close(v, 9.0, 1e-12), "{v}");
}

#[test]
fn integral_variable_is_scoped() {
    let mut s = session();
    ev_with(&mut s, "let x = 100");
    let v = s.eval("∫(0, 2) x dx").unwrap().to_f64();
    assert!(close(v, 2.0, 1e-12));
    assert_eq!(ev_with(&mut s, "x"), "100");
}

// ----- formatting ---------------------------------------------------------------

#[test]
fn format_exact_and_approx() {
    let v = session().eval("1/3").unwrap();
    assert_eq!(format_value(&v, &FormatOptions::default()), "1/3");
    assert_eq!(format_value(&v, &FormatOptions::approx(4)), "0.3333");
    let v = session().eval("[1/2, (1, 1/4)]").unwrap();
    assert_eq!(
        format_value(&v, &FormatOptions::approx(3)),
        "[0.5, (1, 0.25)]"
    );
}

#[test]
fn format_float_uses_digits() {
    let v = session().eval("π").unwrap();
    assert_eq!(
        format_value(&v, &FormatOptions::default()),
        "≈3.14159265359"
    );
    assert_eq!(format_value(&v, &FormatOptions::approx(5)), "3.1416");
}

#[test]
fn approx_mode_converts_result() {
    let mut s = session();
    s.opts.approx = true;
    assert_eq!(s.eval_to_string("1/4").unwrap(), "0.25");
    assert_eq!(s.eval_to_string("[1/2, 2]").unwrap(), "[0.5, 2]");
}

#[test]
fn format_misc_values() {
    assert_eq!(ev("true"), "true");
    assert_eq!(ev("\"hi\""), "\"hi\"");
    assert_eq!(ev("{1 > 2: 1}"), "undefined");
}

// ----- statement classification ------------------------------------------------

fn classify_src(src: &str, env: &Env) -> StatementInfo {
    classify(&parse_statement(src).unwrap(), env)
}

#[test]
fn classify_definitions() {
    let env = Env::new();
    let i = classify_src("a := 3", &env);
    assert_eq!(
        i.kind,
        StatementKind::Definition {
            name: "a".into(),
            params: vec![]
        }
    );
    let i = classify_src("f(x) := x^2 + a", &env);
    assert_eq!(
        i.kind,
        StatementKind::Definition {
            name: "f".into(),
            params: vec!["x".into()]
        }
    );
    assert_eq!(i.free_vars, vec!["a".to_string()]);
    let i = classify_src("g(x, y) := x y", &env);
    assert_eq!(
        i.kind,
        StatementKind::Definition {
            name: "g".into(),
            params: vec!["x".into(), "y".into()]
        }
    );
    assert!(i.free_vars.is_empty());
}

#[test]
fn classify_point_definition() {
    let i = classify_src("P := (1, 2)", &Env::new());
    assert!(matches!(i.kind, StatementKind::Definition { ref name, .. } if name == "P"));
}

#[test]
fn classify_explicit_y() {
    let i = classify_src("y = x^2", &Env::new());
    assert_eq!(i.kind, StatementKind::ExplicitY);
    assert_eq!(i.free_vars, vec!["x".to_string()]);
    assert_eq!(i.expr.unwrap().to_string(), "x^2");
}

#[test]
fn classify_explicit_y_reversed() {
    let i = classify_src("sin(x) = y", &Env::new());
    assert_eq!(i.kind, StatementKind::ExplicitY);
}

#[test]
fn classify_explicit_x() {
    let i = classify_src("x = y^2 - 1", &Env::new());
    assert_eq!(i.kind, StatementKind::ExplicitX);
    let i = classify_src("x = 3", &Env::new());
    assert_eq!(i.kind, StatementKind::ExplicitX);
}

#[test]
fn classify_implicit() {
    let i = classify_src("x^2 + y^2 = 4", &Env::new());
    assert_eq!(i.kind, StatementKind::Implicit);
    assert_eq!(i.free_vars, vec!["x".to_string(), "y".to_string()]);
    let i = classify_src("y = x + y^2", &Env::new());
    assert_eq!(i.kind, StatementKind::Implicit);
}

#[test]
fn classify_implicit_split_names() {
    let i = classify_src("xy = 1", &Env::new());
    assert_eq!(i.kind, StatementKind::Implicit);
    assert_eq!(i.free_vars, vec!["x".to_string(), "y".to_string()]);
}

#[test]
fn classify_polar() {
    let i = classify_src("r = θ", &Env::new());
    assert_eq!(i.kind, StatementKind::Polar);
    let i = classify_src("r = 1 + cos(theta)", &Env::new());
    assert_eq!(i.kind, StatementKind::Polar);
    assert_eq!(i.free_vars, vec!["θ".to_string()]);
}

#[test]
fn classify_inequalities() {
    let i = classify_src("x^2 + y^2 < 4", &Env::new());
    assert_eq!(
        i.kind,
        StatementKind::Inequality {
            ops: vec![crate::syntax::CmpOp::Lt]
        }
    );
    let i = classify_src("y <= sin(x)", &Env::new());
    assert!(matches!(i.kind, StatementKind::Inequality { .. }));
    let i = classify_src("1 < 2", &Env::new());
    assert_eq!(i.kind, StatementKind::Expression);
}

#[test]
fn classify_parametric_and_point() {
    let i = classify_src("(cos t, sin t)", &Env::new());
    assert_eq!(i.kind, StatementKind::Parametric { param: "t".into() });
    let i = classify_src("(1, 2)", &Env::new());
    assert_eq!(i.kind, StatementKind::Point);
}

#[test]
fn classify_expression_kinds() {
    let i = classify_src("1 + 2", &Env::new());
    assert_eq!(i.kind, StatementKind::Expression);
    let i = classify_src("x^2", &Env::new());
    assert_eq!(i.kind, StatementKind::ExplicitY);
    let i = classify_src("a -> a + 1", &Env::new());
    assert_eq!(i.kind, StatementKind::Action);
    let i = classify_src("y1 ~ m x1 + b", &Env::new());
    assert_eq!(i.kind, StatementKind::Regression);
    let i = classify_src("let q = 2", &Env::new());
    assert_eq!(i.kind, StatementKind::Script);
}

#[test]
fn classify_restrictions_and_deps() {
    let mut env = Env::new();
    env.define("a", Value::num(Number::from(2))).unwrap();
    let i = classify_src("y = a x {0 < x < 2}", &env);
    assert_eq!(i.kind, StatementKind::ExplicitY);
    assert_eq!(i.restrictions.len(), 1);
    assert_eq!(i.deps, vec!["a".to_string()]);
    assert_eq!(i.free_vars, vec!["x".to_string()]);
}

#[test]
fn classify_bound_variables_not_free() {
    let i = classify_src("y = ∑(k = 1, 5) k x", &Env::new());
    assert_eq!(i.free_vars, vec!["x".to_string()]);
    let i = classify_src("y = [k x for k = [1..3]]", &Env::new());
    assert_eq!(i.free_vars, vec!["x".to_string()]);
}

#[test]
fn exec_statement_outcomes() {
    let mut env = Env::new();
    let opts = EvalOptions::default();
    let st = parse_statement("a := 1/2").unwrap();
    let o = exec_statement(&st, &mut env, &opts).unwrap();
    assert!(matches!(o, Outcome::Defined { ref name, .. } if name == "a"));
    let st = parse_statement("y = a x").unwrap();
    assert!(matches!(
        exec_statement(&st, &mut env, &opts).unwrap(),
        Outcome::Relation(_)
    ));
    let st = parse_statement("a + 1").unwrap();
    assert_eq!(
        exec_statement(&st, &mut env, &opts).unwrap(),
        Outcome::Value(Value::num(Number::rational(3, 2).unwrap()))
    );
}

#[test]
fn session_eval_relation_is_error() {
    let mut s = session();
    assert!(s.eval("y := x^2").is_err());
}

// ----- resolver -------------------------------------------------------------------

#[test]
fn resolver_supplies_names() {
    let mut s = session();
    s.env.set_resolver(Some(Arc::new(|name: &str| {
        (name == "node").then(|| Value::num(Number::from(42)))
    })));
    assert_eq!(ev_with(&mut s, "node + 1"), "43");
    ev_with(&mut s, "node2 := 1");
    assert_eq!(ev_with(&mut s, "node2"), "1");
}

#[test]
fn resolver_used_after_globals() {
    let mut s = session();
    s.env.set_resolver(Some(Arc::new(|name: &str| {
        (name == "a").then(|| Value::num(Number::from(1)))
    })));
    ev_with(&mut s, "a := 5");
    assert_eq!(ev_with(&mut s, "a"), "5");
}

#[test]
fn eval_expr_is_read_only() {
    let env = Env::new();
    let e = parse_expr("2 + 3").unwrap();
    assert_eq!(
        eval_expr(&e, &env, &EvalOptions::default())
            .unwrap()
            .to_string(),
        "5"
    );
}

// ----- plot fast path -----------------------------------------------------------

fn plot(src: &str, vars: &[&str], env: &Env) -> PlotFn {
    let e = parse_expr(src).unwrap();
    compile_plot_fn(&e, vars, env, &EvalOptions::default()).unwrap()
}

#[test]
fn plot_fn_basic() {
    let f = plot("sin(x) * x^2", &["x"], &Env::new());
    assert!(f.is_compiled());
    assert!(close(f.eval1(2.0), 2f64.sin() * 4.0, 1e-15));
}

#[test]
fn plot_fn_two_vars() {
    let f = plot("x^2 + y^2 - 4", &["x", "y"], &Env::new());
    assert!(f.is_compiled());
    assert_eq!(f.eval2(2.0, 0.0), 0.0);
    assert!(f.eval(&[1.0]).is_nan());
}

#[test]
fn plot_fn_uses_globals_and_inlines_functions() {
    let mut s = session();
    ev_with(&mut s, "a := 3");
    ev_with(&mut s, "f(t) := a t + 1");
    let f = plot("f(x)^2", &["x"], &s.env);
    assert!(f.is_compiled());
    assert_eq!(f.eval1(1.0), 16.0);
}

#[test]
fn plot_fn_split_and_implicit_names() {
    let f = plot("xy - 1", &["x", "y"], &Env::new());
    assert!(f.is_compiled());
    assert_eq!(f.eval2(2.0, 3.0), 5.0);
}

#[test]
fn plot_fn_piecewise_and_restriction() {
    let f = plot("{x < 0: -x, x}", &["x"], &Env::new());
    assert_eq!(f.eval1(-3.0), 3.0);
    assert_eq!(f.eval1(2.0), 2.0);
    let g = plot("(x^2 {0 < x < 2})", &["x"], &Env::new());
    assert_eq!(g.eval1(1.0), 1.0);
    assert!(g.eval1(3.0).is_nan());
}

#[test]
fn plot_fn_odd_root_negative_base() {
    let f = plot("x^(1/3)", &["x"], &Env::new());
    assert!(close(f.eval1(-8.0), -2.0, 1e-12));
    let g = plot("x^(1/2)", &["x"], &Env::new());
    assert!(g.eval1(-4.0).is_nan());
}

#[test]
fn plot_fn_degree_mode() {
    let e = parse_expr("sin(x)").unwrap();
    let opts = EvalOptions {
        angle: AngleUnit::Degrees,
        ..Default::default()
    };
    let f = compile_plot_fn(&e, &["x"], &Env::new(), &opts).unwrap();
    assert!(close(f.eval1(30.0), 0.5, 1e-12));
}

#[test]
fn plot_fn_polar_theta() {
    let f = plot("1 + cos(theta)", &["θ"], &Env::new());
    assert!(close(f.eval1(0.0), 2.0, 1e-15));
}

#[test]
fn plot_fn_fallback_for_sums() {
    let f = plot("∑(k = 1, 3) k x", &["x"], &Env::new());
    assert!(!f.is_compiled());
    assert!(close(f.eval1(2.0), 12.0, 1e-12));
}

#[test]
fn plot_fn_fallback_recursive_fn() {
    let mut s = session();
    ev_with(&mut s, "fn sq(t) { t * t }");
    let f = plot("sq(x) + 1", &["x"], &s.env);
    assert!(!f.is_compiled());
    assert_eq!(f.eval1(3.0), 10.0);
}

#[test]
fn plot_fn_unknown_name_is_error() {
    let e = parse_expr("x + qq").unwrap();
    let err = compile_plot_fn(&e, &["x"], &Env::new(), &EvalOptions::default()).unwrap_err();
    assert!(matches!(err.kind, ErrorKind::UnknownName(_)));
}

#[test]
fn plot_fn_domain_errors_are_nan() {
    let f = plot("ln(x)", &["x"], &Env::new());
    assert!(f.eval1(-1.0).is_nan());
    let g = plot("sqrt(x)", &["x"], &Env::new());
    assert!(g.eval1(-1.0).is_nan());
}

#[test]
fn plot_fn_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>(_: &T) {}
    let f = plot("x", &["x"], &Env::new());
    assert_send_sync(&f);
    let closure = f.into_fn1();
    assert_send_sync(&closure);
    let h = std::thread::spawn(move || closure(4.0));
    assert_eq!(h.join().unwrap(), 4.0);
}

#[test]
fn plot_fn_matches_evaluator() {
    let mut s = session();
    let exprs = [
        "x^3 - 2x + 1",
        "exp(-x^2/2) / sqrt(2π)",
        "atan(x) + floor(x) mod 3",
        "|x - 1| + 2^x",
        "tanh(x) cosh(x)",
    ];
    for src in exprs {
        let f = plot(src, &["x"], &s.env);
        assert!(f.is_compiled(), "{src}");
        for &x in &[-1.5, 0.25, 2.0] {
            ev_with(&mut s, &format!("let x = {x}"));
            let want = s.eval(src).unwrap().to_f64();
            s.env.remove("x");
            let got = f.eval1(x);
            assert!(close(got, want, 1e-12), "{src} at {x}: {got} vs {want}");
        }
    }
}

#[test]
fn plot_fn_speed() {
    let f = plot("sin(x)*x^2", &["x"], &Env::new());
    assert!(f.is_compiled());
    let start = std::time::Instant::now();
    let mut acc = 0.0;
    for i in 0..100_000 {
        acc += f.eval1(i as f64 * 1e-4);
    }
    let elapsed = start.elapsed();
    assert!(acc.is_finite());
    eprintln!("plot_fn_speed: 100k evaluations in {elapsed:?}");
    let limit = if cfg!(debug_assertions) { 2000 } else { 50 };
    assert!(
        elapsed.as_millis() < limit,
        "100k evaluations took {elapsed:?}"
    );
}

// ----- misc -----------------------------------------------------------------------

#[test]
fn float64_mode_evaluates_in_f64() {
    let mut s = session();
    s.opts.float64 = true;
    let v = s.eval("1/3 + π").unwrap();
    assert!(matches!(v, Value::Number(Number::F64(_))));
}

#[test]
fn env_define_get_remove() {
    let mut env = Env::new();
    env.define("pi", Value::num(Number::from(3))).unwrap_err();
    env.define("k", Value::num(Number::from(3))).unwrap();
    assert!(env.contains("k"));
    assert_eq!(env.names().count(), 1);
    assert!(env.remove("k").is_some());
    assert!(env.get("k").is_none());
}

#[test]
fn value_equality_and_type_names() {
    assert_eq!(Value::num(Number::from(1)), Value::num(Number::from(1)));
    assert_ne!(Value::Bool(true), Value::num(Number::from(1)));
    assert_eq!(Value::Undefined.type_name(), "undefined");
    assert_eq!(Value::List(vec![]).type_name(), "list");
}

#[test]
fn multi_statement_console_input() {
    let mut s = session();
    assert_eq!(ev_with(&mut s, "a := 2; b := 3; a b"), "6");
}

// ----- definition operator ----------------------------------------------------

#[test]
fn plain_eq_is_an_equation_by_default() {
    let env = Env::new();
    assert_eq!(classify_src("a = 3", &env).kind, StatementKind::Equation);
    assert_eq!(classify_src("x = 3", &env).kind, StatementKind::ExplicitX);
    assert_eq!(
        classify_src("f(x) = x^2", &env).kind,
        StatementKind::Implicit
    );
    let mut s = session();
    let e = s.eval("a = 3").unwrap_err();
    assert!(e.message.contains(":="), "{}", e.message);
    assert!(!s.env.contains("a"));
}

#[test]
fn colon_eq_defines() {
    let mut s = session();
    assert_eq!(ev_with(&mut s, "a := 3"), "3");
    assert_eq!(ev_with(&mut s, "f(x, y) := a x + y"), "f(x, y) = a * x + y");
    assert_eq!(ev_with(&mut s, "f(2, 1)"), "7");
    assert_eq!(ev_with(&mut s, "b ≔ a + 1"), "4");
}

#[test]
fn colon_eq_needs_definition_head() {
    let e = err("a + 1 := 3");
    assert_eq!(e.kind, ErrorKind::Parse);
    let e = err("f(x, x) := 1");
    assert_eq!(e.kind, ErrorKind::Parse);
}

#[test]
fn eq_mode_keeps_equals_definitions() {
    let mut s = session();
    s.parse.definition_op = crate::syntax::DefinitionOp::Eq;
    assert_eq!(ev_with(&mut s, "a = 3"), "3");
    assert_eq!(ev_with(&mut s, "f(x) = x + a"), "f(x) = x + a");
    assert_eq!(ev_with(&mut s, "f(1)"), "4");
    assert!(matches!(s.run("y = x^2").unwrap()[0], Outcome::Relation(_)));
    assert!(matches!(s.run("x = 2").unwrap()[0], Outcome::Relation(_)));
    // `:=` still defines.
    assert_eq!(ev_with(&mut s, "c := 5"), "5");
    // Script blocks: `=` defines globals / reassigns lets as before.
    ev_with(
        &mut s,
        "let t = 0\nfor k in [1..3] { t = t + k\n total = t }\nt",
    );
    assert_eq!(ev_with(&mut s, "total"), "6");
}

#[test]
fn script_reassignment_of_let_with_equals() {
    assert_eq!(ev("let v = 1\nv = v + 1\nv"), "2");
    // In a script an `=` that is not a reassignment is an error ...
    let prog = crate::syntax::parse_program("let v = 1\nw = 2").unwrap();
    let e = run_program(&prog, &mut Env::new(), &EvalOptions::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Definition);
    // ... and in a block of a session too; at session top level it is a
    // relation.
    let mut s = session();
    let e = s.eval("let v = 1\nif true { w = 2 }").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Definition);
    assert!(matches!(s.run("w = 2").unwrap()[0], Outcome::Relation(_)));
}

#[test]
fn script_top_level_colon_eq_defines_global() {
    let mut s = session();
    ev_with(&mut s, "let k = 2\ng := k * 10\nh(x) := x + k");
    assert_eq!(ev_with(&mut s, "g"), "20");
    assert_eq!(ev_with(&mut s, "h(1)"), "3");
    assert!(!s.env.contains("k"));
}

#[test]
fn definition_prints_colon_eq() {
    let st = parse_statement("f(x) := x^2 {x > 0}").unwrap();
    assert_eq!(st.to_string(), "f(x) := x^2 {x > 0}");
}

// ----- review regressions (PR #10) ---------------------------------------------

#[test]
fn r01_deep_nested_lists_are_an_error_not_a_crash() {
    let h = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let mut s = session();
            s.eval("let v = [1]\nfor i in range(1, 2000) { v = [v] }\nv")
                .unwrap_err()
                .kind
        })
        .unwrap();
    assert_eq!(h.join().unwrap(), ErrorKind::DepthLimit);
    // Moderate nesting is fine.
    assert_eq!(ev("[[[[1]]]]"), "[[[[1]]]]");
}

#[test]
fn r02_plot_split_with_builtin_piece() {
    let f = plot("sinx", &["x"], &Env::new());
    assert!(close(f.eval1(0.5), 0.5f64.sin(), 1e-12));
}

#[test]
fn r03_plot_error_in_condition_is_not_false() {
    // sqrt(-1) is an error: the result is NaN, not the `otherwise` branch.
    let f = plot("{sqrt(x) > 1: 1, 2}", &["x"], &Env::new());
    assert!(f.is_compiled());
    assert!(f.eval1(-1.0).is_nan());
    assert_eq!(f.eval1(4.0), 1.0);
    assert_eq!(f.eval1(0.25), 2.0);
    // An *undefined* condition counts as false.
    let g = plot("{(x {x > 0}) > 1: 1, 2}", &["x"], &Env::new());
    assert_eq!(g.eval1(-1.0), 2.0);
}

#[test]
fn r04_list_piecewise_is_lazy() {
    // The `otherwise` branch would fail but no element needs it.
    assert_eq!(ev("{[1, 2] > 0: 1, 1/0}"), "[1, 1]");
    // A branch nobody selects is not evaluated either.
    assert_eq!(ev("{[1, 2] < 0: 1/0, 5}"), "[5, 5]");
    // Later conditions are not evaluated once every element is resolved.
    assert_eq!(ev("{[1, 2] > 0: 7, 1/0 > 0: 8}"), "[7, 7]");
    assert_eq!(ev("{[-1, 2] > 0: 7, [1, 1] > 0: 8}"), "[8, 7]");
}

#[test]
fn r05_restriction_mask_length_mismatch_is_error() {
    let e = err("([1, 2, 3] {[1, 2] > 0})");
    assert_eq!(e.kind, ErrorKind::Other);
    assert_eq!(ev("(5 {[1, -1] > 0})"), "[5, undefined]");
}

#[test]
fn r06_approx_mode_stores_exact_definitions() {
    let mut s = session();
    s.opts.approx = true;
    assert_eq!(s.eval_to_string("a := 1/3").unwrap(), "0.333333333333");
    s.opts.approx = false;
    assert_eq!(ev_with(&mut s, "a"), "1/3");
}

#[test]
fn r07_fallback_booleans_are_one_and_zero() {
    let f = plot("[x][1] > 0", &["x"], &Env::new());
    assert!(!f.is_compiled());
    assert_eq!(f.eval1(1.0), 1.0);
    assert_eq!(f.eval1(-1.0), 0.0);
}

#[test]
fn r08_inlined_bodies_do_not_see_plot_vars() {
    let mut s = session();
    ev_with(&mut s, "f(t) := t + x");
    // The general evaluator cannot resolve `x` in the body either.
    assert!(s.eval("let x = 1\nf(2)").is_err());
    let e = parse_expr("f(x)").unwrap();
    let r = compile_plot_fn(&e, &["x"], &s.env, &EvalOptions::default());
    assert!(matches!(r.unwrap_err().kind, ErrorKind::UnknownName(_)));
}

#[test]
fn r09_plot_infinities_are_errors() {
    for src in ["1/x", "ln(x)", "x^-1", "1/(1/x)", "x mod 0"] {
        let f = plot(src, &["x"], &Env::new());
        assert!(f.eval1(0.0).is_nan(), "{src}");
    }
    let f = plot("1/0 + x", &["x"], &Env::new());
    assert!(f.eval1(1.0).is_nan());
}

#[test]
fn r10_plot_odd_root_needs_exact_exponent() {
    let f = plot("x^(1/3)", &["x"], &Env::new());
    assert!(close(f.eval1(-8.0), -2.0, 1e-12));
    let g = plot("x^0.3333333333", &["x"], &Env::new());
    assert!(g.eval1(-8.0).is_nan());
    let h = plot("x^(2/3)", &["x"], &Env::new());
    assert!(close(h.eval1(-8.0), 4.0, 1e-12));
}

#[test]
fn r11_plot_variables_are_never_globals() {
    for name in ["x", "y", "r", "t", "theta"] {
        let e = err(&format!("{name} := 3"));
        assert_eq!(e.kind, ErrorKind::Definition, "{name}");
        assert!(e.message.contains("plot variable"), "{}", e.message);
    }
    let mut env = Env::new();
    assert!(env.define("t", Value::num(Number::from(1))).is_err());
    let i = classify_src("x + 1", &env);
    assert_eq!(i.free_vars, vec!["x".to_string()]);
}

#[test]
fn r12_multi_statement_run_routes_outcomes() {
    let mut s = session();
    let out = s.run("let k = 2\na := k\ny = x^2\nk + a").unwrap();
    assert_eq!(out.len(), 4);
    assert!(matches!(out[1], Outcome::Defined { ref name, .. } if name == "a"));
    assert!(matches!(out[2], Outcome::Relation(_)));
    assert_eq!(out[3], Outcome::Value(Value::num(Number::from(4))));
    // `let` locals persist across inputs of the session.
    assert_eq!(ev_with(&mut s, "k * 10"), "20");
}

#[test]
fn r13_script_function_definition_is_global_and_captures() {
    let mut s = session();
    assert_eq!(ev_with(&mut s, "let q = 1\nf(x) := x + q\nf(2)"), "3");
    ev_with(&mut s, "q = 100");
    assert_eq!(ev_with(&mut s, "f(5)"), "6");
    assert!(s.env.contains("f"));
    assert!(!s.env.contains("q"));
}

#[test]
fn r15_vector_ops_charge_per_element() {
    let mut s = session();
    s.opts.max_steps = 2_500;
    let e = s.eval("[1..1000] * [1..1000]").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepLimit);
    let mut s = session();
    s.opts.max_steps = 5_000;
    let e = s.eval("M := [[1..20] for i = [1..20]]\nM^5").unwrap_err();
    assert_eq!(e.kind, ErrorKind::StepLimit);
}

#[test]
fn r16_element_budget_across_comprehensions() {
    let mut s = session();
    s.opts.max_elements = 50_000;
    let e = s.eval("[len([1..10000]) for k = [1..10]]").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Other);
    assert!(e.message.contains("elements"));
}

#[test]
fn r17_split_backtracking_is_fast() {
    let mut s = session();
    ev_with(&mut s, "a := 1");
    ev_with(&mut s, "aa := 2");
    ev_with(&mut s, "aaa := 3");
    let start = std::time::Instant::now();
    let e = s.eval("aaaaaaaaaaaaaaaaaaaaaaq").unwrap_err();
    assert!(matches!(e.kind, ErrorKind::UnknownName(_)));
    assert!(start.elapsed().as_millis() < 500);
}

#[test]
fn r18_min_max_of_empty_list() {
    assert!(matches!(err("min([])").kind, ErrorKind::Number(_)));
    assert!(matches!(err("max([])").kind, ErrorKind::Number(_)));
}

#[test]
fn polar_inequalities_and_angles() {
    let env = Env::new();
    assert!(matches!(
        classify_src("r < 2", &env).kind,
        StatementKind::PolarInequality { .. }
    ));
    assert!(matches!(
        classify_src("0 < θ < π/2", &env).kind,
        StatementKind::PolarInequality { .. }
    ));
    assert_eq!(
        classify_src("θ = π/4", &env).kind,
        StatementKind::PolarAngle
    );
    assert_eq!(classify_src("r = 2θ", &env).kind, StatementKind::Polar);
}

#[test]
fn numeric_calculus_results_are_marked_approximate() {
    assert!(ev("∫(0, 1) x^2 dx").starts_with('≈'));
    assert_eq!(ev("∑(k = 1, 3) k"), "6");
}

#[test]
fn y1_equation_is_a_relation_and_colon_eq_defines() {
    let env = Env::new();
    assert_eq!(classify_src("y1 = x^2", &env).kind, StatementKind::Implicit);
    assert!(matches!(
        classify_src("y1 := x^2", &env).kind,
        StatementKind::Definition { .. }
    ));
}

#[test]
fn definition_returns_value() {
    let mut s = session();
    assert_eq!(ev_with(&mut s, "c := 1/2 + 1/2"), "1");
}
