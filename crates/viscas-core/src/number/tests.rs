use std::cmp::Ordering;

use super::*;

// ---------------------------------------------------------------- helpers

fn n(text: &str) -> Number {
    parse_literal(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn int(v: i64) -> Number {
    Number::from(v)
}

fn rat(p: i64, q: i64) -> Number {
    Number::rational(p, q).unwrap()
}

fn p(digits: u32) -> Precision {
    Precision::digits(digits)
}

const P50: Precision = Precision::DEFAULT;

/// Asserts `|value - expected| <= |expected| · 10^-digits` (absolute when
/// `expected` is zero), with `expected` given as a decimal literal.
#[track_caller]
fn assert_close(value: &Number, expected: &str, digits: u32) {
    let reference = n(expected);
    let diff = (value - &reference).abs();
    let scale = if reference.is_zero() {
        Number::one()
    } else {
        reference.abs()
    };
    let tol = &scale * &int(10).pow(&int(-(digits as i64)), P50).unwrap();
    assert!(
        diff.cmp_real(&tol).unwrap() != Ordering::Greater,
        "value {} differs from {expected} beyond {digits} digits",
        value.to_decimal_string(digits as usize + 5)
    );
}

const PI: &str = "3.14159265358979323846264338327950288419716939937510582097494459230781640628620899862803482534211706798214808651328230665";
const E: &str = "2.71828182845904523536028747135266249775724709369995957496696762772407663035354759457138217852516642742746639193200305992";
const LN2: &str = "0.693147180559945309417232121458176568075500134360255254120680009493393621969694715605863326996418687542001481020570685734";
const SQRT2: &str = "1.41421356237309504880168872420969807856967187537694807317667973799073247846210703885038753432764157273501384623091229702";
const SIN1: &str = "0.841470984807896506652502321630298999622563060798371065672751709991910404391239668948639743543052695854349037907920674293";
const COS1: &str = "0.540302305868139717400936607442976603732310420617922227670097255381100394774471764517951856087183089343571731160030089098";

// ---------------------------------------------------------------- parsing

#[test]
fn parse_integer() {
    assert_eq!(n("42"), int(42));
    assert!(n("42").is_integer());
}

#[test]
fn parse_negative_and_plus() {
    assert_eq!(n("-17"), int(-17));
    assert_eq!(n("+3"), int(3));
}

#[test]
fn parse_big_integer() {
    let v = n("123456789012345678901234567890");
    assert_eq!(v.to_string(), "123456789012345678901234567890");
}

#[test]
fn parse_decimal_is_exact() {
    assert_eq!(n("0.1"), rat(1, 10));
    assert!(matches!(n("0.1"), Number::Rational(_)));
}

#[test]
fn parse_decimal_normalizes_to_integer() {
    assert!(matches!(n("2.50"), Number::Rational(_)));
    assert_eq!(n("2.50"), rat(5, 2));
    assert!(n("3.000").is_integer());
}

#[test]
fn parse_leading_and_trailing_dot() {
    assert_eq!(n(".5"), rat(1, 2));
    assert_eq!(n("5."), int(5));
}

#[test]
fn parse_scientific_exact() {
    assert_eq!(n("1.2e-3"), rat(3, 2500));
    assert_eq!(n("6E3"), int(6000));
    assert_eq!(n("2.5e+2"), int(250));
}

#[test]
fn parse_imaginary() {
    let z = n("2i");
    assert!(z.is_complex());
    assert_eq!(z.re(), int(0));
    assert_eq!(z.im(), int(2));
    assert_eq!(n("i"), Number::i());
    assert_eq!(n("-i"), -Number::i());
    assert_eq!(n("1.5e1i").im(), int(15));
}

#[test]
fn parse_zero_imaginary_collapses() {
    assert!(n("0i").is_real());
}

#[test]
fn parse_digit_separators() {
    assert_eq!(n("1_000_000"), int(1_000_000));
}

#[test]
fn parse_huge_exponent_gives_float() {
    let v = n("1e100000");
    assert!(matches!(v, Number::Float(_)));
    assert_eq!(v.to_decimal_string(5), "1e100000");
    let w = n("1e-5000");
    assert!(matches!(w, Number::Float(_)));
    assert_eq!(w.to_decimal_string(3), "1e-5000");
}

#[test]
fn parse_errors() {
    for bad in [
        "", "abc", "1.2.3", "e5", "1e", "1e+", "--1", ".", "1_", "_1", "1x", "ii",
    ] {
        assert!(
            matches!(parse_literal(bad), Err(NumberError::Parse(_))),
            "{bad:?} should not parse"
        );
    }
}

#[test]
fn from_str_trait() {
    assert!("3/4".parse::<Number>().is_err());
    let w: Number = "0.75".parse().unwrap();
    assert_eq!(w, rat(3, 4));
}

// ---------------------------------------------------------------- exact arithmetic

#[test]
fn rational_addition_stays_exact() {
    assert_eq!(rat(1, 3) + rat(1, 6), rat(1, 2));
    assert!(matches!(rat(1, 3) + rat(2, 3), Number::Integer(_)));
}

#[test]
fn integer_division_gives_rational() {
    let q = int(7).checked_div(&int(2)).unwrap();
    assert_eq!(q, rat(7, 2));
    assert_eq!(int(6).checked_div(&int(3)).unwrap(), int(2));
    assert!(int(6).checked_div(&int(3)).unwrap().is_integer());
}

#[test]
fn rational_normalized() {
    let r = rat(6, -4);
    assert_eq!(r.to_string(), "-3/2");
    assert_eq!(Number::rational(4, 2).unwrap().to_string(), "2");
}

#[test]
fn division_by_zero_errors() {
    assert_eq!(
        int(1).checked_div(&int(0)),
        Err(NumberError::DivisionByZero)
    );
    assert_eq!(
        Number::F64(1.0).checked_div(&Number::F64(0.0)),
        Err(NumberError::DivisionByZero)
    );
    assert_eq!(
        n("1.5").to_float(P50).checked_div(&int(0)),
        Err(NumberError::DivisionByZero)
    );
    assert_eq!(Number::rational(1, 0), Err(NumberError::DivisionByZero));
    assert_eq!(int(0).recip(), Err(NumberError::DivisionByZero));
}

#[test]
fn subtraction_and_negation() {
    assert_eq!(int(3) - int(5), int(-2));
    assert_eq!(-rat(1, 2), rat(-1, 2));
    assert_eq!(rat(1, 2) - rat(1, 2), int(0));
    assert!((rat(1, 2) - rat(1, 2)).is_exact_zero());
}

#[test]
fn multiplication_promotes() {
    assert_eq!(int(3) * rat(1, 3), int(1));
    let f = int(3) * n("0.5").to_float(P50);
    assert!(matches!(f, Number::Float(_)));
    assert_eq!(f, rat(3, 2));
}

#[test]
fn promotion_to_f64() {
    let v = int(1) + Number::F64(0.5);
    assert!(matches!(v, Number::F64(x) if x == 1.5));
    let w = n("0.25").to_float(P50) * Number::F64(2.0);
    assert!(matches!(w, Number::F64(x) if x == 0.5));
}

#[test]
fn f64_overflow_promotes_to_float() {
    let big = Number::F64(1e308);
    let v = &big * &big;
    assert!(matches!(v, Number::Float(_)));
    assert_eq!(v.to_decimal_string(3), "1e616");
}

#[test]
fn float_precision_propagates() {
    let a = int(1).to_float(p(100));
    let b = int(3).to_float(p(20));
    let q = a.checked_div(&b).unwrap();
    let s = q.to_decimal_string(100);
    assert_eq!(s, format!("0.{}", "3".repeat(100)));
}

#[test]
fn big_integer_arithmetic() {
    let a = n("99999999999999999999999999999999");
    let b = &a + &int(1);
    assert_eq!(b.to_string(), "100000000000000000000000000000000");
    assert_eq!((&b * &b).to_string(), format!("1{}", "0".repeat(64)));
}

// ---------------------------------------------------------------- complex arithmetic

#[test]
fn complex_add_mul() {
    let z = int(3) + n("2i");
    let w = int(1) - n("i");
    assert_eq!((&z + &w).to_string(), "4 + i");
    // (3+2i)(1-i) = 3 - 3i + 2i + 2 = 5 - i
    assert_eq!((&z * &w).to_string(), "5 - i");
}

#[test]
fn i_squared_is_minus_one() {
    let v = Number::i() * Number::i();
    assert!(v.is_real());
    assert_eq!(v, int(-1));
}

#[test]
fn complex_division_exact() {
    let z = int(1) + Number::i();
    let w = int(1) - Number::i();
    assert_eq!(z.checked_div(&w).unwrap(), Number::i());
    assert_eq!(int(1).checked_div(&Number::i()).unwrap(), -Number::i());
    let h = Number::complex(int(1), int(1))
        .checked_div(&int(2))
        .unwrap();
    assert_eq!(h.to_string(), "1/2 + i/2");
}

#[test]
fn complex_conj_re_im() {
    let z = Number::complex(int(2), int(-5));
    assert_eq!(z.conj(), Number::complex(int(2), int(5)));
    assert_eq!(z.re(), int(2));
    assert_eq!(z.im(), int(-5));
    assert_eq!((&z * &z.conj()), int(29));
}

#[test]
fn complex_abs_exact_and_numeric() {
    assert_eq!(Number::complex(int(3), int(4)).abs(), int(5));
    assert_close(&Number::complex(int(1), int(1)).abs(), SQRT2, 48);
}

#[test]
fn complex_comparison_errors() {
    assert_eq!(Number::i().cmp_real(&int(1)), Err(NumberError::NotReal));
    assert_eq!(Number::i().partial_cmp(&int(1)), None);
    assert_eq!(Number::i().floor(), Err(NumberError::NotReal));
}

#[test]
fn complex_power_integer() {
    let z = int(1) + Number::i();
    assert_eq!(z.pow(&int(2), P50).unwrap(), n("2i"));
    assert_eq!(z.pow(&int(8), P50).unwrap(), int(16));
    assert_eq!(z.pow(&int(-2), P50).unwrap(), -n("i") * rat(1, 2));
}

// ---------------------------------------------------------------- powers

#[test]
fn zero_pow_zero_is_one() {
    assert_eq!(int(0).pow(&int(0), P50).unwrap(), int(1));
    assert_eq!(
        Number::F64(0.0).pow(&Number::F64(0.0), P50).unwrap(),
        int(1)
    );
}

#[test]
fn zero_pow_negative_errors() {
    assert_eq!(int(0).pow(&int(-1), P50), Err(NumberError::DivisionByZero));
    assert_eq!(
        int(0).pow(&rat(-1, 2), P50),
        Err(NumberError::DivisionByZero)
    );
}

#[test]
fn zero_pow_positive() {
    assert_eq!(int(0).pow(&rat(1, 2), P50).unwrap(), int(0));
    assert_eq!(int(0).pow(&Number::F64(2.5), P50).unwrap(), int(0));
}

#[test]
fn integer_powers_exact() {
    assert_eq!(int(2).pow(&int(10), P50).unwrap(), int(1024));
    assert_eq!(int(2).pow(&int(-3), P50).unwrap(), rat(1, 8));
    assert_eq!(rat(2, 3).pow(&int(-2), P50).unwrap(), rat(9, 4));
    assert_eq!(int(-2).pow(&int(3), P50).unwrap(), int(-8));
}

#[test]
fn big_integer_power() {
    let v = int(2).pow(&int(200), P50).unwrap();
    assert_eq!(
        v.to_string(),
        "1606938044258990275541962092341162602522202993782792835301376"
    );
}

#[test]
fn rational_powers_perfect() {
    assert_eq!(int(8).pow(&rat(1, 3), P50).unwrap(), int(2));
    assert_eq!(int(4).pow(&rat(1, 2), P50).unwrap(), int(2));
    assert_eq!(int(4).pow(&rat(3, 2), P50).unwrap(), int(8));
    assert_eq!(rat(27, 8).pow(&rat(-2, 3), P50).unwrap(), rat(4, 9));
}

#[test]
fn pow_exact_signals_not_exact() {
    assert_eq!(int(2).pow_exact(&rat(1, 2)).unwrap(), None);
    assert_eq!(int(-8).pow_exact(&rat(1, 3)).unwrap(), None);
    assert_eq!(n("1.5").to_float(P50).pow_exact(&int(2)).unwrap(), None);
    assert_eq!(int(4).pow_exact(&rat(1, 2)).unwrap(), Some(int(2)));
}

#[test]
fn pow_exact_huge_is_not_exact() {
    let e = n("100000000000");
    assert_eq!(int(3).pow_exact(&e).unwrap(), None);
    assert_eq!(int(1).pow_exact(&e).unwrap(), Some(int(1)));
    assert_eq!(int(-1).pow_exact(&(&e + &int(1))).unwrap(), Some(int(-1)));
}

#[test]
fn huge_exact_power_falls_back_to_float() {
    let v = int(10).pow(&n("100000000000"), P50).unwrap();
    assert!(matches!(v, Number::Float(_)));
    assert_eq!(v.to_decimal_string(5), "1e100000000000");
}

#[test]
fn negative_base_half_power_is_imaginary() {
    assert_eq!(int(-4).pow(&rat(1, 2), P50).unwrap(), n("2i"));
    assert_eq!(int(-4).pow(&rat(3, 2), P50).unwrap(), n("-8i"));
}

#[test]
fn irrational_power_numeric() {
    assert_close(&int(2).pow(&rat(1, 2), P50).unwrap(), SQRT2, 48);
    assert_close(
        &int(2).pow(&n("2").sqrt(P50).unwrap(), P50).unwrap(),
        "2.66514414269022518865029724987313984827421131371465949283597959336492",
        47,
    );
    assert_close(
        &int(2).pow(&rat(1, 3), P50).unwrap(),
        "1.25992104989487316476721060727822835057025146470150798008197511215529967",
        48,
    );
}

#[test]
fn negative_base_cube_root_principal() {
    // (-8)^(1/3) principal = 1 + √3 i
    let v = int(-8).pow(&rat(1, 3), P50).unwrap();
    assert_close(&v.re(), "1", 45);
    assert_close(
        &v.im(),
        "1.73205080756887729352744634150587236694280525381038062805580697945193",
        45,
    );
}

#[test]
fn nth_root_real_for_odd() {
    assert_eq!(int(-8).nth_root(&int(3), P50).unwrap(), int(-2));
    assert_eq!(int(81).nth_root(&int(4), P50).unwrap(), int(3));
    assert_close(
        &int(-2).nth_root(&int(3), P50).unwrap(),
        "-1.25992104989487316476721060727822835057025146470150798008197511215529967",
        48,
    );
}

#[test]
fn f64_powers() {
    let v = Number::F64(2.0).pow(&Number::F64(0.5), P50).unwrap();
    assert!(matches!(v, Number::F64(x) if (x - std::f64::consts::SQRT_2).abs() < 1e-15));
    let w = Number::F64(-2.0).pow(&Number::F64(3.0), P50).unwrap();
    assert!(matches!(w, Number::F64(x) if x == -8.0));
}

#[test]
fn float_power_integer_exponent() {
    let x = n("1.5").to_float(P50);
    assert_eq!(x.pow(&int(3), P50).unwrap(), rat(27, 8));
    assert_eq!(
        x.pow(&int(-1), P50).unwrap().to_decimal_string(10),
        "0.6666666667"
    );
}

#[test]
fn complex_pow_i_to_i() {
    // i^i = e^(-π/2)
    let v = Number::i().pow(&Number::i(), P50).unwrap();
    assert!(v.is_real() || v.im().is_zero());
    assert_close(
        &v.re(),
        "0.207879576350761908546955619834978770033877841631769608075135883055419877",
        47,
    );
}

// ---------------------------------------------------------------- comparisons and rounding

#[test]
fn ordering_across_kinds() {
    assert!(rat(1, 3) < n("0.34"));
    assert!(int(1) == Number::F64(1.0));
    assert!(rat(1, 2) == n("0.5").to_float(P50));
    assert!(Number::F64(0.1) != rat(1, 10));
    assert_eq!(int(-2).cmp_real(&rat(-3, 2)).unwrap(), Ordering::Less);
    assert_eq!(
        n("2")
            .sqrt(P50)
            .unwrap()
            .cmp_real(&rat(141421, 100000))
            .unwrap(),
        Ordering::Greater
    );
}

#[test]
fn min_max() {
    assert_eq!(int(3).min_real(&rat(5, 2)).unwrap(), rat(5, 2));
    assert_eq!(int(3).max_real(&rat(5, 2)).unwrap(), int(3));
}

#[test]
fn floor_ceil_round_rationals() {
    assert_eq!(rat(7, 2).floor().unwrap(), int(3));
    assert_eq!(rat(-7, 2).floor().unwrap(), int(-4));
    assert_eq!(rat(7, 2).ceil().unwrap(), int(4));
    assert_eq!(rat(-7, 2).ceil().unwrap(), int(-3));
    assert_eq!(rat(5, 2).round().unwrap(), int(3));
    assert_eq!(rat(-5, 2).round().unwrap(), int(-3));
    assert_eq!(rat(-7, 2).trunc().unwrap(), int(-3));
}

#[test]
fn floor_ceil_round_floats() {
    let x = n("-2.5").to_float(P50);
    assert_eq!(x.floor().unwrap(), int(-3));
    assert_eq!(x.ceil().unwrap(), int(-2));
    assert_eq!(x.round().unwrap(), int(-3));
    let tiny = n("1e-30").to_float(P50);
    assert_eq!(tiny.floor().unwrap(), int(0));
    assert_eq!(tiny.ceil().unwrap(), int(1));
    assert_eq!((-tiny).floor().unwrap(), int(-1));
    assert_eq!(Number::F64(2.7).floor().unwrap().to_f64(), 2.0);
}

#[test]
fn abs_and_sign() {
    assert_eq!(rat(-3, 4).abs(), rat(3, 4));
    assert_eq!(int(-9).sign().unwrap(), int(-1));
    assert_eq!(int(0).sign().unwrap(), int(0));
    assert_eq!(rat(2, 7).sign().unwrap(), int(1));
    assert_eq!(Number::complex(int(0), int(3)).sign().unwrap(), Number::i());
}

#[test]
fn math_modulo_sign_of_divisor() {
    assert_eq!(int(7).modulo(&int(3)).unwrap(), int(1));
    assert_eq!(int(-7).modulo(&int(3)).unwrap(), int(2));
    assert_eq!(int(7).modulo(&int(-3)).unwrap(), int(-2));
    assert_eq!(rat(7, 2).modulo(&int(1)).unwrap(), rat(1, 2));
    assert_eq!(rat(-1, 3).modulo(&rat(1, 2)).unwrap(), rat(1, 6));
    assert_eq!(int(5).modulo(&int(0)), Err(NumberError::DivisionByZero));
    let f = Number::F64(-1.5).modulo(&Number::F64(1.0)).unwrap();
    assert!(matches!(f, Number::F64(x) if x == 0.5));
}

#[test]
fn float_modulo() {
    let pi = Number::pi(P50).unwrap();
    let m = (&pi * &int(3)).modulo(&pi).unwrap();
    assert!(
        m.abs().cmp_real(&n("1e-45")).unwrap() == Ordering::Less || {
            let d = (&m - &pi).abs();
            d.cmp_real(&n("1e-45")).unwrap() == Ordering::Less
        }
    );
}

// ---------------------------------------------------------------- number theory

#[test]
fn gcd_lcm_integers() {
    assert_eq!(int(12).gcd(&int(18)).unwrap(), int(6));
    assert_eq!(int(-12).gcd(&int(18)).unwrap(), int(6));
    assert_eq!(int(0).gcd(&int(5)).unwrap(), int(5));
    assert_eq!(int(4).lcm(&int(6)).unwrap(), int(12));
    assert_eq!(int(0).lcm(&int(6)).unwrap(), int(0));
}

#[test]
fn gcd_lcm_rationals() {
    assert_eq!(rat(1, 2).gcd(&rat(1, 3)).unwrap(), rat(1, 6));
    assert_eq!(rat(1, 2).lcm(&rat(1, 3)).unwrap(), int(1));
    assert_eq!(Number::F64(1.5).gcd(&int(2)), Err(NumberError::NotInteger));
    assert_eq!(
        Number::F64(1.0).gcd(&rat(1, 2)),
        Err(NumberError::NotInteger)
    );
    // Integral floats are accepted (float64 mode: `gcd(4, 6)`).
    assert_eq!(
        Number::F64(4.0).gcd(&Number::F64(6.0)).unwrap().to_f64(),
        2.0
    );
    assert!(matches!(Number::F64(4.0).lcm(&int(6)).unwrap(), Number::F64(x) if x == 12.0));
}

#[test]
fn factorial_small() {
    assert_eq!(int(0).factorial(P50).unwrap(), int(1));
    assert_eq!(int(1).factorial(P50).unwrap(), int(1));
    assert_eq!(int(10).factorial(P50).unwrap(), int(3_628_800));
}

#[test]
fn factorial_100() {
    let v = int(100).factorial(P50).unwrap();
    assert_eq!(
        v.to_string(),
        "93326215443944152681699238856266700490715968264381621468592963895217599993229915608941463976156518286253697920827223758251185210916864000000000000000000000000"
    );
    assert_eq!(v.to_decimal_string(5), "9.3326e157");
}

#[test]
fn factorial_large_digit_count() {
    let v = int(1000).factorial(P50).unwrap();
    assert_eq!(v.to_string().len(), 2568);
}

#[test]
fn factorial_errors() {
    assert!(matches!(
        int(-1).factorial(P50),
        Err(NumberError::Domain(_))
    ));
    assert_eq!(n("10000000000").factorial(P50), Err(NumberError::Limit));
}

#[test]
fn ncr_npr() {
    assert_eq!(int(5).ncr(&int(2)).unwrap(), int(10));
    assert_eq!(int(5).ncr(&int(0)).unwrap(), int(1));
    assert_eq!(int(5).ncr(&int(6)).unwrap(), int(0));
    assert_eq!(int(5).ncr(&int(-1)).unwrap(), int(0));
    assert_eq!(int(-3).ncr(&int(2)).unwrap(), int(6));
    assert_eq!(int(5).npr(&int(2)).unwrap(), int(20));
    assert_eq!(int(5).npr(&int(6)).unwrap(), int(0));
    assert_eq!(int(5).npr(&int(0)).unwrap(), int(1));
}

#[test]
fn ncr_large() {
    let v = int(100).ncr(&int(50)).unwrap();
    assert_eq!(v.to_string(), "100891344545564193334812497256");
}

#[test]
fn isqrt_values() {
    assert_eq!(int(0).isqrt().unwrap(), int(0));
    assert_eq!(int(15).isqrt().unwrap(), int(3));
    assert_eq!(int(16).isqrt().unwrap(), int(4));
    let big = n("1000000000000000000000000000000");
    assert_eq!(big.isqrt().unwrap(), n("1000000000000000"));
    assert!(matches!(int(-1).isqrt(), Err(NumberError::Domain(_))));
}

#[test]
fn primality_small() {
    let primes: Vec<i64> = (0..60).filter(|&k| int(k).is_prime().unwrap()).collect();
    assert_eq!(
        primes,
        vec![
            2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59
        ]
    );
    assert!(!int(-7).is_prime().unwrap());
}

#[test]
fn primality_u64_edge_cases() {
    assert!(n("18446744073709551557").is_prime().unwrap()); // largest u64 prime
    assert!(!n("18446744073709551615").is_prime().unwrap());
    // strong pseudoprimes to several bases
    assert!(!int(3_215_031_751).is_prime().unwrap());
    assert!(!n("3825123056546413051").is_prime().unwrap());
    assert!(!int(561).is_prime().unwrap()); // Carmichael
}

#[test]
fn primality_big() {
    // 2^89 - 1 and 2^127 - 1 are Mersenne primes
    assert!(n("618970019642690137449562111").is_prime().unwrap());
    assert!(
        n("170141183460469231731687303715884105727")
            .is_prime()
            .unwrap()
    );
    assert!(
        !n("170141183460469231731687303715884105729")
            .is_prime()
            .unwrap()
    );
    // ψ13 (strong pseudoprime to the first 13 prime bases' predecessor set)
    assert!(!n("3317044064679887385961981").is_prime().unwrap());
}

#[test]
fn primality_requires_integer() {
    assert_eq!(rat(1, 2).is_prime(), Err(NumberError::NotInteger));
}

fn factors(v: &str) -> Vec<(String, u32)> {
    n(v).factorize()
        .unwrap()
        .into_iter()
        .map(|(p, k)| (p.to_string(), k))
        .collect()
}

fn f(list: &[(&str, u32)]) -> Vec<(String, u32)> {
    list.iter().map(|(p, k)| (p.to_string(), *k)).collect()
}

#[test]
fn factorize_small() {
    assert_eq!(factors("1"), vec![]);
    assert_eq!(factors("360"), f(&[("2", 3), ("3", 2), ("5", 1)]));
    assert_eq!(factors("-12"), f(&[("-1", 1), ("2", 2), ("3", 1)]));
    assert_eq!(factors("97"), f(&[("97", 1)]));
}

#[test]
fn factorize_project_euler() {
    assert_eq!(
        factors("600851475143"),
        f(&[("71", 1), ("839", 1), ("1471", 1), ("6857", 1)])
    );
}

#[test]
fn factorize_pollard_u64() {
    // 1000000007 · 998244353
    assert_eq!(
        factors("998244359987710471"),
        f(&[("998244353", 1), ("1000000007", 1)])
    );
    // perfect square of a large prime
    assert_eq!(factors("1000000014000000049"), f(&[("1000000007", 2)]));
}

#[test]
fn factorize_beyond_u64() {
    // 2^64 + 1 = 274177 · 67280421310721
    assert_eq!(
        factors("18446744073709551617"),
        f(&[("274177", 1), ("67280421310721", 1)])
    );
    // 2^128 - 1 = 3 · 5 · 17 · 257 · 641 · 65537 · 274177 · 6700417 · 67280421310721
    assert_eq!(
        factors("340282366920938463463374607431768211455"),
        f(&[
            ("3", 1),
            ("5", 1),
            ("17", 1),
            ("257", 1),
            ("641", 1),
            ("65537", 1),
            ("274177", 1),
            ("6700417", 1),
            ("67280421310721", 1)
        ])
    );
}

#[test]
fn factorize_zero_errors() {
    assert!(matches!(int(0).factorize(), Err(NumberError::Domain(_))));
}

// ---------------------------------------------------------------- constants and elementary functions

#[test]
fn pi_50_digits() {
    let pi = Number::pi(P50).unwrap();
    assert_eq!(
        pi.to_decimal_string(50),
        "3.1415926535897932384626433832795028841971693993751"
    );
}

#[test]
fn pi_200_digits() {
    assert_close(&Number::pi(p(110)).unwrap(), PI, 110);
}

#[test]
fn e_constant() {
    assert_close(&Number::e(P50).unwrap(), E, 49);
    assert_close(&Number::e(p(110)).unwrap(), E, 110);
}

#[test]
fn ln2_constant() {
    assert_close(&int(2).ln(p(110)).unwrap(), LN2, 110);
}

#[test]
fn sqrt_exact_and_numeric() {
    assert_eq!(int(144).sqrt(P50).unwrap(), int(12));
    assert_eq!(rat(9, 16).sqrt(P50).unwrap(), rat(3, 4));
    assert_close(&int(2).sqrt(P50).unwrap(), SQRT2, 49);
    assert_close(&int(2).sqrt(p(110)).unwrap(), SQRT2, 110);
}

#[test]
fn sqrt_negative_is_imaginary() {
    assert_eq!(int(-4).sqrt(P50).unwrap(), n("2i"));
    let v = int(-2).sqrt(P50).unwrap();
    assert!(v.re().is_zero());
    assert_close(&v.im(), SQRT2, 49);
}

#[test]
fn sqrt_complex() {
    assert!(!n("3").sqrt(P50).unwrap().is_exact());
    let z = Number::complex(int(3), int(4));
    assert_eq!(z.sqrt(P50).unwrap(), Number::complex(int(2), int(1)));
    let w = Number::complex(int(3), int(-4));
    assert_eq!(w.sqrt(P50).unwrap(), Number::complex(int(2), int(-1)));
    // √i = (1+i)/√2
    let r = Number::i().sqrt(P50).unwrap();
    assert_close(
        &r.re(),
        "0.707106781186547524400844362104849039284835937688474036588",
        48,
    );
    assert_close(
        &r.im(),
        "0.707106781186547524400844362104849039284835937688474036588",
        48,
    );
    // √(-3-4i) = 1 - 2i
    let m = Number::complex(int(-3), int(-4));
    assert_eq!(m.sqrt(P50).unwrap(), Number::complex(int(1), int(-2)));
}

#[test]
fn exp_values() {
    assert_eq!(int(0).exp(P50).unwrap(), int(1));
    assert_close(&int(1).exp(P50).unwrap(), E, 49);
    assert_close(
        &int(10).exp(P50).unwrap(),
        "22026.4657948067165169579006452842443663535126185567810742354263552252",
        49,
    );
    assert_close(
        &int(-1).exp(P50).unwrap(),
        "0.367879441171442321595523770161460867445811131031767834507836801697461",
        49,
    );
    assert_close(
        &int(1000).exp(P50).unwrap(),
        "1.97007111401704699388887935224332312531693798532384578995280299138506385078E+434",
        48,
    );
}

#[test]
fn exp_overflow_and_underflow() {
    let huge = n("1e30");
    assert_eq!(huge.exp(P50), Err(NumberError::Overflow));
    assert!((-huge).exp(P50).unwrap().is_zero());
}

#[test]
fn ln_values() {
    assert_eq!(int(1).ln(P50).unwrap(), int(0));
    assert_close(&rat(1, 2).ln(P50).unwrap(), &format!("-{LN2}"), 49);
    assert_close(
        &int(10).ln(P50).unwrap(),
        "2.30258509299404568401799145468436420760110148862877297603332790096757",
        49,
    );
    assert!(matches!(int(0).ln(P50), Err(NumberError::Domain(_))));
}

#[test]
fn ln_near_one_keeps_relative_precision() {
    let x = &int(1) + &n("1e-40");
    assert_close(
        &x.ln(P50).unwrap(),
        "9.99999999999999999999999999999999999999950000000000000000000000000000000000000003333333E-41",
        48,
    );
}

#[test]
fn ln_negative_is_complex() {
    let v = int(-1).ln(P50).unwrap();
    assert!(v.re().is_zero());
    assert_close(&v.im(), PI, 49);
    let w = Number::F64(-1.0).ln(P50).unwrap();
    assert_eq!(w.to_complex_f64(), (0.0, std::f64::consts::PI));
}

#[test]
fn ln_complex() {
    // ln(i) = iπ/2
    let v = Number::i().ln(P50).unwrap();
    assert!(v.re().is_zero());
    assert_close(&(v.im() * int(2)), PI, 49);
}

#[test]
fn exp_ln_roundtrip() {
    let x = rat(7, 3);
    let back = x.ln(P50).unwrap().exp(P50).unwrap();
    assert_close(
        &back,
        "2.33333333333333333333333333333333333333333333333333333",
        47,
    );
}

#[test]
fn log10_and_log_base() {
    assert_eq!(int(1000).log10(P50).unwrap(), int(3));
    assert_eq!(rat(1, 100).log10(P50).unwrap(), int(-2));
    assert_eq!(int(8).log(&int(2), P50).unwrap(), int(3));
    assert_eq!(rat(1, 8).log(&rat(1, 2), P50).unwrap(), int(3));
    assert_close(
        &int(2).log10(P50).unwrap(),
        "0.301029995663981195213738894724493026768189881462108541310427461127108",
        49,
    );
    assert_close(&int(3).log(&int(9), P50).unwrap(), "0.5", 49);
    assert!(matches!(
        int(3).log(&int(1), P50),
        Err(NumberError::Domain(_))
    ));
    assert!(matches!(int(0).log10(P50), Err(NumberError::Domain(_))));
}

#[test]
fn sin_cos_tan_values() {
    assert_close(&int(1).sin(P50).unwrap(), SIN1, 49);
    assert_close(&int(1).cos(P50).unwrap(), COS1, 49);
    assert_close(
        &int(1).tan(P50).unwrap(),
        "1.55740772465490223050697480745836017308725077238152003838394660569886",
        49,
    );
    assert_eq!(int(0).sin(P50).unwrap(), int(0));
    assert_eq!(int(0).cos(P50).unwrap(), int(1));
}

#[test]
fn sin_high_precision() {
    assert_close(&int(1).sin(p(110)).unwrap(), SIN1, 110);
    assert_close(&int(1).cos(p(110)).unwrap(), COS1, 110);
}

#[test]
fn sin_large_argument() {
    assert_close(
        &int(100).sin(P50).unwrap(),
        "-0.506365641109758793656557610459785432065032721290657323443392473594358",
        48,
    );
}

#[test]
fn sin_tiny_argument() {
    assert_close(
        &n("1e-30").sin(P50).unwrap(),
        "9.99999999999999999999999999999999999999999999999999999999999833333333333E-31",
        48,
    );
}

#[test]
fn sin_pi_is_tiny_but_accurate() {
    // sin(π_50) = π - π_50 to first order: relative accuracy must survive.
    let pi = Number::pi(P50).unwrap();
    let s = pi.sin(P50).unwrap();
    let exact_pi = n(PI);
    let expected = (&exact_pi - &pi).to_decimal_string(30);
    assert_close(&s, &expected, 25);
}

#[test]
fn cos_half_pi_relative_accuracy() {
    let hp = Number::pi(P50).unwrap().checked_div(&int(2)).unwrap();
    let c = hp.cos(P50).unwrap();
    let expected = (&(n(PI) * rat(1, 2)) - &hp).to_decimal_string(30);
    assert_close(&c, &expected, 25);
}

#[test]
fn pythagorean_identity() {
    let x = rat(123, 7);
    let s = x.sin(p(60)).unwrap();
    let c = x.cos(p(60)).unwrap();
    assert_close(&(&s * &s + &c * &c), "1", 58);
}

#[test]
fn inverse_trig() {
    // asin(1/2) = π/6
    assert_close(
        &rat(1, 2).asin(P50).unwrap(),
        "0.523598775598298873077107230546583814032861566562517636829157432051303",
        49,
    );
    assert_close(
        &int(1).asin(P50).unwrap(),
        &(n(PI) * rat(1, 2)).to_decimal_string(70),
        49,
    );
    // acos(1/2) = π/3
    assert_close(
        &rat(1, 2).acos(P50).unwrap(),
        &(n(PI) * rat(1, 3)).to_decimal_string(70),
        49,
    );
    assert_close(&int(-1).acos(P50).unwrap(), PI, 49);
    assert_eq!(int(1).acos(P50).unwrap(), int(0));
    assert_close(
        &rat(1, 2).atan(P50).unwrap(),
        "0.463647609000806116214256231461214402028537054286120263810933088720198",
        49,
    );
    // atan(1) = π/4
    assert_close(
        &int(1).atan(P50).unwrap(),
        &(n(PI) * rat(1, 4)).to_decimal_string(70),
        49,
    );
    assert_close(
        &n("1e20").atan(P50).unwrap(),
        &(n(PI) * rat(1, 2) - n("1e-20")).to_decimal_string(70),
        49,
    );
}

#[test]
fn inverse_trig_domain_errors() {
    assert!(matches!(int(2).asin(P50), Err(NumberError::Domain(_))));
    assert!(matches!(rat(-3, 2).acos(P50), Err(NumberError::Domain(_))));
    assert!(matches!(
        Number::F64(1.5).asin(P50),
        Err(NumberError::Domain(_))
    ));
    assert_eq!(Number::i().asin(P50), Err(NumberError::NotReal));
}

#[test]
fn acos_near_one() {
    // acos(1 - 1e-30) ≈ √(2e-30) · (1 + ...) = 1.41421356237309504880e-15...
    let x = &int(1) - &n("1e-30");
    let v = x.acos(P50).unwrap();
    assert_close(&v, "1.4142135623730950488016887242097e-15", 25);
}

#[test]
fn atan2_quadrants() {
    let pi = n(PI);
    assert_close(
        &Number::atan2(&int(1), &int(1), P50).unwrap(),
        &(&pi * &rat(1, 4)).to_decimal_string(70),
        49,
    );
    assert_close(
        &Number::atan2(&int(1), &int(-1), P50).unwrap(),
        &(&pi * &rat(3, 4)).to_decimal_string(70),
        49,
    );
    assert_close(
        &Number::atan2(&int(-1), &int(-1), P50).unwrap(),
        &(&pi * &rat(-3, 4)).to_decimal_string(70),
        49,
    );
    assert_close(
        &Number::atan2(&int(-1), &int(1), P50).unwrap(),
        &(&pi * &rat(-1, 4)).to_decimal_string(70),
        49,
    );
    assert_close(&Number::atan2(&int(0), &int(-1), P50).unwrap(), PI, 49);
    assert_close(
        &Number::atan2(&int(2), &int(0), P50).unwrap(),
        &(&pi * &rat(1, 2)).to_decimal_string(70),
        49,
    );
    assert_eq!(Number::atan2(&int(0), &int(0), P50).unwrap(), int(0));
    assert_eq!(Number::atan2(&int(0), &int(5), P50).unwrap(), int(0));
}

#[test]
fn hyperbolic_functions() {
    assert_close(
        &int(1).sinh(P50).unwrap(),
        "1.17520119364380145688238185059560081515571798133409587022956541301331",
        49,
    );
    assert_close(
        &int(1).cosh(P50).unwrap(),
        "1.54308063481524377847790562075706168260152911236586370473740221471077",
        49,
    );
    assert_close(
        &rat(1, 2).tanh(P50).unwrap(),
        "0.462117157260009758502318483643672548730289280330113038552731815838081",
        49,
    );
    assert_eq!(int(0).cosh(P50).unwrap(), int(1));
    assert_close(&int(1000).tanh(P50).unwrap(), "1", 49);
    assert_close(&int(-1000).tanh(P50).unwrap(), "-1", 49);
}

#[test]
fn sinh_tiny_relative_precision() {
    assert_close(
        &n("1e-30").sinh(P50).unwrap(),
        "1.0000000000000000000000000000000000000000000000000000000000016666666e-30",
        48,
    );
    assert_close(
        &n("1e-30").tanh(P50).unwrap(),
        "9.99999999999999999999999999999999999999999999999999999999999666666666e-31",
        48,
    );
}

#[test]
fn inverse_hyperbolic() {
    assert_close(
        &int(1).asinh(P50).unwrap(),
        "0.881373587019543025232609324979792309028160328261635410753295608653377",
        49,
    );
    assert_close(
        &int(2).acosh(P50).unwrap(),
        "1.31695789692481670862504634730796844402698197146751647976847225692046",
        49,
    );
    assert_close(
        &rat(1, 2).atanh(P50).unwrap(),
        "0.549306144334054845697622618461262852323745278911374725867347166818747",
        49,
    );
    assert_close(
        &rat(-1, 2).atanh(P50).unwrap(),
        "-0.549306144334054845697622618461262852323745278911374725867347166818747",
        49,
    );
    assert_close(
        &int(-1).asinh(P50).unwrap(),
        "-0.881373587019543025232609324979792309028160328261635410753295608653377",
        49,
    );
    assert_eq!(int(1).acosh(P50).unwrap(), int(0));
    assert!(matches!(rat(1, 2).acosh(P50), Err(NumberError::Domain(_))));
    assert!(matches!(int(1).atanh(P50), Err(NumberError::Domain(_))));
}

#[test]
fn inverse_roundtrips() {
    let x = rat(3, 10);
    let pairs: Vec<(Number, Number)> = vec![
        (x.sin(P50).unwrap().asin(P50).unwrap(), x.clone()),
        (x.cos(P50).unwrap().acos(P50).unwrap(), x.clone()),
        (x.tan(P50).unwrap().atan(P50).unwrap(), x.clone()),
        (x.sinh(P50).unwrap().asinh(P50).unwrap(), x.clone()),
        (x.tanh(P50).unwrap().atanh(P50).unwrap(), x.clone()),
        (int(3).cosh(P50).unwrap().acosh(P50).unwrap(), int(3)),
    ];
    for (got, want) in pairs {
        assert_close(&got, &want.to_decimal_string(60), 47);
    }
}

#[test]
fn complex_exp_euler() {
    // e^(iπ) = -1
    let v = (Number::i() * Number::pi(P50).unwrap()).exp(P50).unwrap();
    assert_close(&v.re(), "-1", 48);
    assert!(v.im().abs().cmp_real(&n("1e-48")).unwrap() == Ordering::Less);
}

#[test]
fn complex_trig() {
    // sin(i) = i·sinh(1); cos(i) = cosh(1)
    let s = Number::i().sin(P50).unwrap();
    assert!(s.re().is_zero());
    assert_close(
        &s.im(),
        "1.17520119364380145688238185059560081515571798133409587022956541301331",
        49,
    );
    let c = Number::i().cos(P50).unwrap();
    assert_close(
        &c.re(),
        "1.54308063481524377847790562075706168260152911236586370473740221471077",
        49,
    );
    let t = Number::i().tan(P50).unwrap();
    assert_close(
        &t.im(),
        "0.761594155955764888119458282604793590412768597257936551596810500121",
        48,
    );
}

#[test]
fn f64_fast_path_functions() {
    let x = Number::F64(1.0);
    assert!(matches!(x.sin(P50).unwrap(), Number::F64(v) if v == 1f64.sin()));
    assert!(matches!(x.exp(P50).unwrap(), Number::F64(v) if v == 1f64.exp()));
    assert!(matches!(Number::F64(2.0).sqrt(P50).unwrap(), Number::F64(v) if v == 2f64.sqrt()));
    assert!(matches!(
        Number::F64(0.5).atanh(P50).unwrap(),
        Number::F64(_)
    ));
}

#[test]
fn f64_exp_overflow_falls_back_to_float() {
    let v = Number::F64(1000.0).exp(P50).unwrap();
    assert!(matches!(v, Number::Float(_)));
    assert_eq!(v.to_decimal_string(5), "1.9701e434");
}

#[test]
fn float_inputs_keep_their_precision() {
    let x = int(2).to_float(p(100));
    let s = x.sqrt(P50).unwrap();
    assert_close(&s, SQRT2, 99);
}

#[test]
fn trig_huge_argument_errors() {
    assert_eq!(n("1e1000000").sin(P50), Err(NumberError::Overflow));
}

#[test]
fn pow_float_large_exponent_precision() {
    // 1.0001^10000 = e^(10000 ln 1.0001)
    let v = rat(10001, 10000).pow(&n("10000.5"), P50).unwrap();
    let expected = (n("10000.5") * rat(10001, 10000).ln(p(80)).unwrap())
        .exp(p(80))
        .unwrap();
    assert_close(&v, &expected.to_decimal_string(70), 48);
}

// ---------------------------------------------------------------- formatting

#[test]
fn exact_display() {
    assert_eq!(int(-7).to_string(), "-7");
    assert_eq!(rat(1, 3).to_string(), "1/3");
    assert_eq!(rat(-1, 3).to_string(), "-1/3");
    assert_eq!(n("2i").to_string(), "2i");
    assert_eq!(n("-i").to_string(), "-i");
    assert_eq!((int(3) - n("2i")).to_string(), "3 - 2i");
    assert_eq!(
        (rat(1, 2) + Number::i() * rat(3, 2)).to_string(),
        "1/2 + 3i/2"
    );
}

#[test]
fn decimal_display_rationals() {
    assert_eq!(rat(1, 3).to_decimal_string(10), "0.3333333333");
    assert_eq!(rat(2, 3).to_decimal_string(5), "0.66667");
    assert_eq!(rat(1, 8).to_decimal_string(10), "0.125");
    assert_eq!(rat(-1, 8).to_decimal_string(2), "-0.13");
    assert_eq!(int(0).to_decimal_string(5), "0");
}

#[test]
fn decimal_display_trims_zeros() {
    assert_eq!(n("2.5000").to_decimal_string(10), "2.5");
    assert_eq!(n("100").to_float(P50).to_decimal_string(10), "100");
    assert_eq!(n("0.1").to_float(P50).to_decimal_string(20), "0.1");
}

#[test]
fn decimal_display_scientific() {
    assert_eq!(n("1.5e-7").to_decimal_string(10), "1.5e-7");
    assert_eq!(n("0.00001").to_decimal_string(10), "0.00001");
    assert_eq!(n("0.000001").to_decimal_string(10), "1e-6");
    assert_eq!(n("12345678").to_decimal_string(4), "1.235e7");
    assert_eq!(n("1e25").to_decimal_string(30), "1e25");
    assert_eq!(
        n("-6.02214076e23").to_float(P50).to_decimal_string(10),
        "-6.02214076e23"
    );
}

#[test]
fn decimal_display_rounding_carry() {
    assert_eq!(n("9.9996").to_decimal_string(4), "10");
    assert_eq!(n("0.099999").to_decimal_string(3), "0.1");
}

#[test]
fn f64_display() {
    assert_eq!(Number::F64(0.1).to_string(), "0.1");
    assert_eq!(Number::F64(1.0 / 3.0).to_string(), "0.333333333333333");
    assert_eq!(Number::F64(-2.0).to_string(), "-2");
    assert_eq!(Number::F64(1e-300).to_string(), "1e-300");
    assert_eq!(
        Number::F64(0.1).to_decimal_string(17),
        "0.10000000000000001"
    );
}

#[test]
fn float_display_uses_precision() {
    let third = int(1).to_float(p(20)).checked_div(&int(3)).unwrap();
    assert_eq!(third.to_string(), format!("0.{}", "3".repeat(20)));
}

#[test]
fn complex_decimal_display() {
    let z = Number::complex(rat(1, 3), rat(-2, 3));
    assert_eq!(z.to_decimal_string(4), "0.3333 - 0.6667i");
    let v = int(-2).sqrt(P50).unwrap();
    assert_eq!(v.to_decimal_string(6), "1.41421i");
}

#[test]
fn big_float_display() {
    let v = int(2).to_float(P50).pow(&int(10000), P50).unwrap();
    assert_eq!(v.to_decimal_string(10), "1.995063117e3010");
}

#[test]
fn to_f64_conversions() {
    assert_eq!(rat(1, 4).to_f64(), 0.25);
    assert_eq!(rat(1, 3).to_f64(), 1.0 / 3.0);
    assert_eq!(n("0.1").to_f64(), 0.1);
    assert_eq!(Number::pi(P50).unwrap().to_f64(), std::f64::consts::PI);
    assert_eq!(int(-5).to_f64(), -5.0);
    assert!(Number::i().to_f64().is_nan());
    assert_eq!((int(1) + n("2i")).to_complex_f64(), (1.0, 2.0));
    assert_eq!(n("1e-400").to_f64(), 0.0);
    assert_eq!(n("1e400").to_f64(), f64::INFINITY);
}

// ---------------------------------------------------------------- BigFloat internals

#[test]
fn bigfloat_roundtrip_f64() {
    for x in [
        0.0,
        1.0,
        -1.5,
        0.1,
        1e-310,
        1.7976931348623157e308,
        123456.789,
    ] {
        let b = BigFloat::from_f64(x, 53).unwrap();
        assert_eq!(b.to_f64(), x);
    }
    assert!(BigFloat::from_f64(f64::NAN, 53).is_none());
}

#[test]
fn bigfloat_round_half_even() {
    // 2^53 + 1 is not representable in 53 bits: ties to even -> 2^53
    let x = BigFloat::from_bigint(&(num_bigint::BigInt::from(1u64 << 53) + 1), 53);
    assert_eq!(x.to_f64(), 9007199254740992.0);
    let y = BigFloat::from_bigint(&(num_bigint::BigInt::from(1u64 << 53) + 3), 53);
    assert_eq!(y.to_f64(), 9007199254740996.0);
}

#[test]
fn bigfloat_add_disparate_magnitudes() {
    let big = BigFloat::from_i64(1, 100).mul_pow2(1_000_000);
    let small = BigFloat::from_i64(1, 100);
    assert_eq!(big.add(&small), big);
    let sum = BigFloat::from_i64(1, 100).add(&BigFloat::from_i64(1, 100).mul_pow2(-90));
    assert_ne!(sum, BigFloat::from_i64(1, 100));
}

#[test]
fn bigfloat_cmp() {
    let a = BigFloat::from_i64(3, 64);
    let b = BigFloat::from_i64(-5, 64);
    assert_eq!(a.cmp_value(&b), Ordering::Greater);
    assert_eq!(b.cmp_value(&a), Ordering::Less);
    assert_eq!(a.cmp_value(&a.clone()), Ordering::Equal);
    assert_eq!(BigFloat::zero(10).cmp_value(&b), Ordering::Greater);
}

#[test]
fn bigfloat_sqrt_correctly_rounded() {
    let two = BigFloat::from_i64(2, 53);
    assert_eq!(two.sqrt().unwrap().to_f64(), std::f64::consts::SQRT_2);
    assert!(two.neg().sqrt().is_none());
    assert_eq!(
        BigFloat::from_i64(49, 10).sqrt().unwrap(),
        BigFloat::from_i64(7, 10)
    );
}

#[test]
fn bigfloat_div_correctly_rounded() {
    let one = BigFloat::from_i64(1, 53);
    let three = BigFloat::from_i64(3, 53);
    assert_eq!(one.div(&three).unwrap().to_f64(), 1.0 / 3.0);
    let ten = BigFloat::from_i64(10, 53);
    assert_eq!(one.div(&ten).unwrap().to_f64(), 0.1);
    assert!(one.div(&BigFloat::zero(53)).is_none());
}

#[test]
fn bigfloat_powi() {
    let x = BigFloat::from_i64(3, 64);
    assert_eq!(
        x.powi(&BigInt::from(4)).unwrap(),
        BigFloat::from_i64(81, 64)
    );
    assert_eq!(x.powi(&BigInt::from(-1)).unwrap().to_f64(), 1.0 / 3.0);
    assert_eq!(
        BigFloat::from_i64(2, 64).powi(&"10000000000000000000000".parse().unwrap()),
        Err(NumberError::Overflow)
    );
}

#[test]
fn bigfloat_floor_variants() {
    let x = BigFloat::from_f64(-3.25, 64).unwrap();
    assert_eq!(x.floor().unwrap(), BigInt::from(-4));
    assert_eq!(x.ceil().unwrap(), BigInt::from(-3));
    assert_eq!(x.trunc().unwrap(), BigInt::from(-3));
    assert_eq!(x.round().unwrap(), BigInt::from(-3));
    assert!(BigFloat::from_i64(1, 10).mul_pow2(1 << 40).floor().is_err());
}

#[test]
fn bigfloat_constants_display() {
    assert_eq!(
        BigFloat::pi(Precision::digits(30).bits())
            .unwrap()
            .to_string(),
        "3.14159265358979323846264338328"
    );
    assert_eq!(
        Number::Float(BigFloat::e(Precision::digits(25).bits()).unwrap()).to_decimal_string(20),
        "2.7182818284590452354"
    );
}

#[test]
fn precision_conversions() {
    assert_eq!(Precision::DEFAULT.get(), 50);
    assert_eq!(Precision::digits(50).bits(), 167);
    assert_eq!(Precision::from_bits(167).get(), 50);
    assert_eq!(Precision::digits(0).get(), 1);
    assert_eq!(Precision::default(), Precision::DEFAULT);
}

#[test]
fn predicates() {
    assert!(int(3).is_exact());
    assert!(!Number::F64(3.0).is_exact());
    assert!(Number::F64(3.0).is_integral());
    assert!(!rat(1, 2).is_integral());
    assert!(n("2i").is_exact());
    assert!(!int(0).to_float(P50).is_exact_zero());
    assert!(int(0).to_float(P50).is_zero());
    assert_eq!(
        Number::f64(f64::NAN),
        Err(NumberError::Domain("non-finite float"))
    );
    assert_eq!(rat(-1, 2).signum_i32(), Some(-1));
    assert_eq!(Number::i().signum_i32(), None);
}

#[test]
fn error_display() {
    assert_eq!(NumberError::DivisionByZero.to_string(), "division by zero");
    assert_eq!(
        NumberError::Parse("x".into()).to_string(),
        "invalid number literal: x"
    );
}

// ---------------------------------------------------------------- review regressions

const SQRT_PI: &str =
    "1.7724538509055160272981674833411451827975494561223871282138077898529112845910322";

/// Every `F64` must be finite (invariant), recursively for complex parts.
fn assert_finite_f64(v: &Number) {
    match v {
        Number::F64(x) => assert!(x.is_finite(), "non-finite F64 {x}"),
        Number::Complex(c) => {
            assert_finite_f64(c.re());
            assert_finite_f64(c.im());
        }
        _ => {}
    }
}

#[test]
fn regression_f64_with_out_of_range_operand() {
    // #1: 10^400 + F64(1.0) used to panic (to_f64 = inf).
    let big = n("1e400");
    for v in [
        &big + &Number::F64(1.0),
        &big - &Number::F64(1.0),
        &big * &Number::F64(2.0),
        big.checked_div(&Number::F64(2.0)).unwrap(),
        Number::F64(1.0).checked_div(&big).unwrap(),
    ] {
        assert_finite_f64(&v);
        let _ = v.to_string();
    }
    assert_eq!((&big * &Number::F64(2.0)).to_decimal_string(3), "2e400");
    assert_eq!(
        big.cmp_real(&Number::F64(1e308)).unwrap(),
        Ordering::Greater
    );
}

#[test]
fn regression_f64_pow_overflow() {
    // #2: F64^F64 overflow used to panic in the BigFloat fallback.
    let v = Number::F64(10.0).pow(&Number::F64(400.5), P50).unwrap();
    assert_finite_f64(&v);
    assert!(matches!(v, Number::Float(_)));
    assert_eq!(v.to_decimal_string(4), "3.162e400");
}

#[test]
fn regression_f64_results_stay_finite() {
    // #3: modulo and negative-base powers produced inf / -inf F64 values.
    let m = Number::F64(1e300).modulo(&Number::F64(1e-10)).unwrap();
    assert_finite_f64(&m);
    assert!(m.cmp_real(&int(0)).unwrap() != Ordering::Less);
    assert!(m.cmp_real(&Number::F64(1e-10)).unwrap() == Ordering::Less);
    let p = Number::F64(-10.0).pow(&Number::F64(400.0), P50).unwrap();
    assert_finite_f64(&p);
    assert_eq!(p.to_decimal_string(3), "1e400");
    let q = Number::F64(-10.0).pow(&Number::F64(401.0), P50).unwrap();
    assert_eq!(q.to_decimal_string(3), "-1e401");
    let r = Number::F64(1e-300)
        .checked_div(&Number::F64(1e300))
        .unwrap();
    assert_finite_f64(&r);
}

#[test]
fn regression_sign_of_complex_never_panics() {
    // #4: sign of a complex whose modulus underflowed used to panic.
    let z = Number::complex(Number::F64(1e-200), Number::F64(1e-200));
    let s = z.sign().unwrap();
    assert_close(&s.re(), "0.70710678118654752", 14);
    assert_close(&s.im(), "0.70710678118654752", 14);
}

#[test]
fn regression_complex_modulus_no_underflow() {
    // #5: |z|, z/w and ln z squared f64 parts.
    let z = Number::complex(Number::F64(3e-200), Number::F64(4e-200));
    assert_close(&z.abs(), "5e-200", 14);
    let big = Number::complex(Number::F64(3e200), Number::F64(4e200));
    assert_close(&big.abs(), "5e200", 14);
    let q = Number::F64(1.0).checked_div(&z).unwrap();
    assert_close(&q.re(), "1.2e199", 14);
    assert_close(&q.im(), "-1.6e199", 14);
    let l = z.ln(P50).unwrap();
    assert_close(
        &l.re(),
        "-458.907580686375036428997531603646653880694696",
        13,
    );
}

#[test]
fn regression_parse_extreme_exponent() {
    // #6: exp10.abs() overflowed for i64::MIN.
    assert!(parse_literal("1e-9223372036854775808").is_err());
    assert!(parse_literal("1e9223372036854775807").is_err());
}

#[test]
fn regression_complex_precision_threaded() {
    // #7: complex abs/sqrt/sign ignored the requested precision.
    let z = Number::complex(int(1), int(1));
    assert_close(&z.abs_prec(p(100)), SQRT2, 99);
    let r = Number::complex(int(1), int(2)).sqrt(p(100)).unwrap();
    assert_close(
        &r.re(),
        "1.27201964951406896425242246173749149171560804184009624861664038253929757553606801183038421",
        85,
    );
    let s = z.sign_prec(p(100)).unwrap();
    assert_close(&(s.re() * int(2)), SQRT2, 99);
}

#[test]
fn regression_round_extreme_floats() {
    // #8: round() materialized 2^|exponent| bits.
    assert_eq!(n("1e-100000").round().unwrap(), int(0));
    assert_eq!((-n("1e-100000")).round().unwrap(), int(0));
    let x = BigFloat::from_i64(1, 10).mul_pow2(-(1 << 40));
    assert_eq!(x.round().unwrap(), BigInt::from(0));
    assert_eq!(
        BigFloat::from_i64(3, 10).mul_pow2(-1).round().unwrap(),
        BigInt::from(2)
    );
}

#[test]
fn regression_complex_power_bounded() {
    // #9: complex integer powers had no size bound.
    let z = Number::complex(int(1), int(1));
    assert_eq!(z.pow_exact(&n("100000000000")).unwrap(), None);
    let v = z.pow(&n("100000000000"), P50).unwrap();
    assert!(!v.is_exact());
    let w = Number::complex(Number::F64(0.6), Number::F64(0.8));
    let u = w.pow(&n("1000000000000"), P50).unwrap();
    assert_finite_f64(&u);
    assert_close(&u.abs(), "1", 3);
}

#[test]
fn regression_ncr_npr_limits() {
    // #10
    assert_eq!(n("1e30").ncr(&n("1e20")), Err(NumberError::Limit));
    assert_eq!(n("1e30").npr(&n("1e10")), Err(NumberError::Limit));
    assert_eq!(
        n("1e30").ncr(&int(2)).unwrap(),
        n("499999999999999999999999999999500000000000000000000000000000")
    );
}

#[test]
fn regression_prime_and_factor_limits() {
    // #11
    let huge = int(2).pow(&int(5000), P50).unwrap() + int(1);
    assert_eq!(huge.is_prime(), Err(NumberError::Limit));
    assert_eq!(huge.factorize(), Err(NumberError::Limit));
    // Product of two 32-bit primes: Pollard-Brent within its iteration budget.
    assert_eq!(
        factors("18446743979220271189"),
        f(&[("4294967279", 1), ("4294967291", 1)])
    );
}

#[test]
fn regression_trig_argument_cap() {
    // #12
    assert_eq!(n("1e30000").sin(P50), Err(NumberError::Overflow));
    assert!(n("1e3000").sin(P50).is_ok());
}

#[test]
fn regression_bigfloat_pow_huge_exponent() {
    // #13: no ln at millions of bits for absurd exponents.
    let two = BigFloat::from_i64(2, 64);
    let third = BigFloat::from_ratio(&1.into(), &3.into(), 64);
    let y = BigFloat::from_i64(1, 64).mul_pow2(1_000_000).add(&third);
    assert_eq!(two.pow(&y), Err(NumberError::Overflow));
    let half = BigFloat::from_ratio(&1.into(), &2.into(), 64);
    assert!(half.pow(&y).unwrap().is_zero());
    assert_eq!(BigFloat::one(64).pow(&y).unwrap(), BigFloat::one(64));
}

#[test]
fn regression_cmp_rational_magnitude_tie() {
    // #15: equal magnitudes with huge exponents compared as Equal.
    let Number::Float(f) = n("1e5000") else {
        panic!("expected a float");
    };
    let r = f.to_rational();
    let bigger = &r + &BigRational::from_integer(1.into());
    let smaller = &r - &BigRational::from_integer(1.into());
    assert_eq!(f.cmp_rational(&r), Ordering::Equal);
    assert_eq!(f.cmp_rational(&bigger), Ordering::Less);
    assert_eq!(f.cmp_rational(&smaller), Ordering::Greater);
    assert_eq!(f.neg().cmp_rational(&-&bigger), Ordering::Greater);
}

#[test]
fn regression_float_modulo_sign() {
    // #16
    let a = n("-7.5").to_float(P50);
    assert_eq!(a.modulo(&int(2)).unwrap(), rat(1, 2));
    assert_eq!(n("7.5").to_float(P50).modulo(&int(-2)).unwrap(), rat(-1, 2));
    let pi = Number::pi(P50).unwrap();
    let m = (-&pi).modulo(&int(1)).unwrap();
    assert!(m.cmp_real(&int(0)).unwrap() != Ordering::Less);
    assert!(m.cmp_real(&int(1)).unwrap() == Ordering::Less);
    assert_close(
        &m,
        "0.85840734641020676153735661672049711580283060062489",
        45,
    );
}

#[test]
fn regression_format_extreme_exponent() {
    // #17: formatting near the exponent limit must not panic.
    let x = BigFloat::from_i64(3, 64).mul_pow2(1 << 59);
    let s = Number::Float(x).to_decimal_string(5);
    assert!(s.contains('e'), "{s}");
}

#[test]
fn regression_precision_clamped() {
    // #18
    assert_eq!(Precision::digits(u32::MAX).get(), Precision::MAX_DIGITS);
    assert_eq!(
        Precision::from_bits(u64::MAX),
        Precision::digits(Precision::MAX_DIGITS)
    );
    let Number::Float(b) = int(1).to_float(Precision::from_bits(100)) else {
        panic!("expected a float");
    };
    assert_eq!(b.prec(), 100);
    assert_eq!(
        Number::Float(b).working_prec(Precision::digits(1)).bits(),
        100
    );
}

#[test]
fn regression_literal_digit_cap() {
    // #19
    let long = "1".repeat(200_000);
    assert_eq!(parse_literal(&long), Err(NumberError::Limit));
    assert!(parse_literal(&"7".repeat(1000)).is_ok());
}

#[test]
fn interrupt_flag_cancels() {
    // #20
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let flag = Arc::new(AtomicBool::new(true));
    {
        let _guard = install_interrupt(flag.clone());
        assert_eq!(int(5000).factorial(P50), Err(NumberError::Interrupted));
        assert_eq!(int(3).ln(p(1000)), Err(NumberError::Interrupted));
        assert_eq!(
            n("618970019642690137449562111").is_prime(),
            Err(NumberError::Interrupted)
        );
        assert_eq!(
            factors_result("1208925819535464337504999"),
            Err(NumberError::Interrupted)
        );
    }
    // Guard dropped: computations run again.
    assert!(int(3).ln(P50).is_ok());
    assert_eq!(check_interrupt(), Ok(()));
}

fn factors_result(v: &str) -> NumResult<Vec<(BigInt, u32)>> {
    n(v).factorize()
}

#[test]
fn integral_floats_accepted_as_integers() {
    assert_eq!(Number::F64(5.0).factorial(P50).unwrap(), int(120));
    assert_eq!(
        int(5).to_float(P50).ncr(&Number::F64(2.0)).unwrap(),
        int(10)
    );
    assert_eq!(Number::F64(7.0).is_prime(), Ok(true));
    assert_eq!(Number::F64(16.0).isqrt().unwrap(), int(4));
    assert_eq!(Number::F64(2.5).ncr(&int(1)), Err(NumberError::NotInteger));
}

#[test]
fn gamma_and_non_integer_factorial() {
    assert_close(&rat(1, 2).gamma(P50).unwrap(), SQRT_PI, 48);
    assert_close(
        &rat(1, 2).factorial(P50).unwrap(),
        "0.8862269254527580136490837416705725913987747280611935641069038949264556422955161",
        48,
    );
    let minus_two_sqrt_pi = (n(SQRT_PI) * int(-2)).to_decimal_string(70);
    assert_close(&rat(-1, 2).gamma(P50).unwrap(), &minus_two_sqrt_pi, 47);
    // Γ(1/3)·Γ(2/3) = 2π/√3
    let prod = rat(1, 3).gamma(P50).unwrap() * rat(2, 3).gamma(P50).unwrap();
    assert_close(
        &prod,
        "3.6275987284684357011881565152843114645681324961854811511397698707762463622527078",
        47,
    );
    assert_eq!(int(6).gamma(P50).unwrap(), int(120));
    assert_close(
        &n("10.5").to_float(p(80)).gamma(P50).unwrap(),
        "1133278.38894878556733457416558889247556029830827515977660872341452948339005600",
        77,
    );
    assert!(matches!(int(0).gamma(P50), Err(NumberError::Domain(_))));
    assert!(matches!(int(-3).gamma(P50), Err(NumberError::Domain(_))));
    assert!(matches!(
        Number::F64(-2.0).gamma(P50),
        Err(NumberError::Domain(_))
    ));
}

#[test]
fn gamma_f64_path() {
    let g = Number::F64(0.1).gamma(P50).unwrap();
    assert!(matches!(g, Number::F64(x) if (x - 9.513507698668732).abs() < 1e-12));
    let h = Number::F64(-2.5).gamma(P50).unwrap();
    assert!(matches!(h, Number::F64(x) if (x + 0.9453087204829417).abs() < 1e-13));
    let big = Number::F64(170.5).gamma(P50).unwrap();
    assert!(matches!(big, Number::F64(x) if (x / 5.56209241456e305 - 1.0).abs() < 1e-10));
    let over = Number::F64(200.5).gamma(P50).unwrap();
    assert!(matches!(over, Number::Float(_)));
    assert_eq!(
        Number::F64(2.5)
            .factorial(P50)
            .unwrap()
            .to_decimal_string(9),
        "3.32335097"
    );
}

#[test]
fn inexact_zero_imaginary_collapses() {
    let z = Number::complex(Number::F64(1.5), Number::F64(0.0));
    assert!(matches!(z, Number::F64(x) if x == 1.5));
    let w = Number::complex(int(1), int(0).to_float(P50));
    assert!(w.is_real());
}
