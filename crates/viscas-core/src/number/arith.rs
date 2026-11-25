//! Arithmetic with automatic promotion, comparisons and rounding.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::ops::{Add, Mul, Neg, Sub};

use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

use super::{BigFloat, Complex, NumResult, Number, NumberError, Precision};

/// Two real operands coerced to a common kind.
enum RealPair<'a> {
    Int(&'a BigInt, &'a BigInt),
    Rat(Cow<'a, BigRational>, Cow<'a, BigRational>),
    Float(Cow<'a, BigFloat>, Cow<'a, BigFloat>),
    F64(f64, f64),
}

fn rank(n: &Number) -> u8 {
    match n {
        Number::Integer(_) => 0,
        Number::Rational(_) => 1,
        Number::Float(_) => 2,
        Number::F64(_) => 3,
        Number::Complex(_) => 4,
    }
}

fn as_rat(n: &Number) -> Cow<'_, BigRational> {
    match n {
        Number::Integer(i) => Cow::Owned(BigRational::from_integer(i.clone())),
        Number::Rational(r) => Cow::Borrowed(r),
        _ => unreachable!("exact operand expected"),
    }
}

fn as_float(n: &Number, bits: u64) -> Cow<'_, BigFloat> {
    match n {
        Number::Float(f) => Cow::Borrowed(f),
        Number::Integer(i) => Cow::Owned(BigFloat::from_bigint(i, bits)),
        Number::Rational(r) => Cow::Owned(BigFloat::from_ratio(r.numer(), r.denom(), bits)),
        Number::F64(x) => Cow::Owned(f64_big(*x)),
        Number::Complex(_) => unreachable!("real operand expected"),
    }
}

/// Coerces two real numbers to their common kind (the least precise one).
fn real_pair<'a>(a: &'a Number, b: &'a Number) -> RealPair<'a> {
    match rank(a).max(rank(b)) {
        0 => match (a, b) {
            (Number::Integer(x), Number::Integer(y)) => RealPair::Int(x, y),
            _ => unreachable!(),
        },
        1 => RealPair::Rat(as_rat(a), as_rat(b)),
        2 => {
            let bits = match (a, b) {
                (Number::Float(f), _) | (_, Number::Float(f)) => f.prec(),
                _ => unreachable!(),
            };
            RealPair::Float(as_float(a, bits), as_float(b, bits))
        }
        3 => {
            let (x, y) = (a.to_f64(), b.to_f64());
            if x.is_finite() && y.is_finite() {
                RealPair::F64(x, y)
            } else {
                // An exact or Float operand outside the f64 range: compute
                // with 53-bit floats instead of producing inf/NaN.
                RealPair::Float(as_float(a, 53), as_float(b, 53))
            }
        }
        _ => unreachable!("real operands expected"),
    }
}

/// Wraps an `f64` result, falling back to a 53-bit [`BigFloat`] computation
/// on overflow so that `F64` values stay finite.
fn f64_or(x: f64, fallback: impl FnOnce() -> BigFloat) -> Number {
    if x.is_finite() {
        Number::F64(x)
    } else {
        Number::Float(fallback())
    }
}

fn f64_big(x: f64) -> BigFloat {
    BigFloat::from_f64(x, 53).expect("finite")
}

fn real_add(a: &Number, b: &Number) -> Number {
    match real_pair(a, b) {
        RealPair::Int(x, y) => Number::Integer(x + y),
        RealPair::Rat(x, y) => Number::from_ratio(x.as_ref() + y.as_ref()),
        RealPair::Float(x, y) => Number::Float(x.add(&y)),
        RealPair::F64(x, y) => f64_or(x + y, || f64_big(x).add(&f64_big(y))),
    }
}

fn real_sub(a: &Number, b: &Number) -> Number {
    match real_pair(a, b) {
        RealPair::Int(x, y) => Number::Integer(x - y),
        RealPair::Rat(x, y) => Number::from_ratio(x.as_ref() - y.as_ref()),
        RealPair::Float(x, y) => Number::Float(x.sub(&y)),
        RealPair::F64(x, y) => f64_or(x - y, || f64_big(x).sub(&f64_big(y))),
    }
}

fn real_mul(a: &Number, b: &Number) -> Number {
    match real_pair(a, b) {
        RealPair::Int(x, y) => Number::Integer(x * y),
        RealPair::Rat(x, y) => Number::from_ratio(x.as_ref() * y.as_ref()),
        RealPair::Float(x, y) => Number::Float(x.mul(&y)),
        RealPair::F64(x, y) => f64_or(x * y, || f64_big(x).mul(&f64_big(y))),
    }
}

fn real_div(a: &Number, b: &Number) -> NumResult<Number> {
    if b.is_zero() {
        return Err(NumberError::DivisionByZero);
    }
    Ok(match real_pair(a, b) {
        RealPair::Int(x, y) => Number::from_ratio(BigRational::new(x.clone(), y.clone())),
        RealPair::Rat(x, y) => Number::from_ratio(x.as_ref() / y.as_ref()),
        RealPair::Float(x, y) => Number::Float(x.div(&y).expect("non-zero")),
        RealPair::F64(x, y) => f64_or(x / y, || f64_big(x).div(&f64_big(y)).expect("non-zero")),
    })
}

fn parts(n: &Number) -> (Cow<'_, Number>, Cow<'_, Number>) {
    match n {
        Number::Complex(c) => (Cow::Borrowed(&c.re), Cow::Borrowed(&c.im)),
        other => (Cow::Borrowed(other), Cow::Owned(Number::zero())),
    }
}

fn num_add(a: &Number, b: &Number) -> Number {
    if a.is_real() && b.is_real() {
        return real_add(a, b);
    }
    let (ar, ai) = parts(a);
    let (br, bi) = parts(b);
    Number::complex(real_add(&ar, &br), real_add(&ai, &bi))
}

fn num_sub(a: &Number, b: &Number) -> Number {
    if a.is_real() && b.is_real() {
        return real_sub(a, b);
    }
    let (ar, ai) = parts(a);
    let (br, bi) = parts(b);
    Number::complex(real_sub(&ar, &br), real_sub(&ai, &bi))
}

fn num_mul(a: &Number, b: &Number) -> Number {
    match (a, b) {
        (Number::Complex(_), Number::Complex(_)) => {
            let (ar, ai) = parts(a);
            let (br, bi) = parts(b);
            let re = real_sub(&real_mul(&ar, &br), &real_mul(&ai, &bi));
            let im = real_add(&real_mul(&ar, &bi), &real_mul(&ai, &br));
            Number::complex(re, im)
        }
        (Number::Complex(c), r) | (r, Number::Complex(c)) => {
            Number::complex(real_mul(&c.re, r), real_mul(&c.im, r))
        }
        _ => real_mul(a, b),
    }
}

fn num_neg(a: &Number) -> Number {
    match a {
        Number::Integer(n) => Number::Integer(-n),
        Number::Rational(r) => Number::Rational(-r),
        Number::Float(f) => Number::Float(f.neg()),
        Number::F64(x) => Number::F64(-x),
        Number::Complex(c) => Number::Complex(Box::new(Complex {
            re: num_neg(&c.re),
            im: num_neg(&c.im),
        })),
    }
}

macro_rules! impl_binop {
    ($trait:ident, $method:ident, $f:ident) => {
        impl $trait<&Number> for &Number {
            type Output = Number;
            fn $method(self, rhs: &Number) -> Number {
                $f(self, rhs)
            }
        }
        impl $trait<Number> for Number {
            type Output = Number;
            fn $method(self, rhs: Number) -> Number {
                $f(&self, &rhs)
            }
        }
        impl $trait<&Number> for Number {
            type Output = Number;
            fn $method(self, rhs: &Number) -> Number {
                $f(&self, rhs)
            }
        }
        impl $trait<Number> for &Number {
            type Output = Number;
            fn $method(self, rhs: Number) -> Number {
                $f(self, &rhs)
            }
        }
    };
}

impl_binop!(Add, add, num_add);
impl_binop!(Sub, sub, num_sub);
impl_binop!(Mul, mul, num_mul);

impl Neg for Number {
    type Output = Number;
    fn neg(self) -> Number {
        num_neg(&self)
    }
}

impl Neg for &Number {
    type Output = Number;
    fn neg(self) -> Number {
        num_neg(self)
    }
}

impl PartialEq for Number {
    /// Numeric equality across kinds (`1 == 1.0`, `1/2 == 0.5`).
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Number::Complex(a), Number::Complex(b)) => a.re == b.re && a.im == b.im,
            (Number::Complex(_), _) | (_, Number::Complex(_)) => false,
            _ => self.cmp_real(other) == Ok(Ordering::Equal),
        }
    }
}

impl PartialOrd for Number {
    /// Total order on reals; `None` when a complex number is involved.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.cmp_real(other).ok()
    }
}

impl Number {
    /// Exact division; errors on division by zero.
    pub fn checked_div(&self, other: &Number) -> NumResult<Number> {
        if other.is_zero() {
            return Err(NumberError::DivisionByZero);
        }
        if self.is_real() && other.is_real() {
            return real_div(self, other);
        }
        if has_f64_part(self) || has_f64_part(other) {
            // Avoid under/overflow of c² + d² in f64: divide with 53-bit
            // floats (unbounded exponent range), then convert back.
            let q = f64_parts_to_big(self).checked_div(&f64_parts_to_big(other))?;
            return Ok(q.into_f64_kind());
        }
        match other {
            Number::Complex(c) => {
                // (a+bi)/(c+di) = (a+bi)(c-di) / (c²+d²)
                let den = real_add(&real_mul(&c.re, &c.re), &real_mul(&c.im, &c.im));
                let num = num_mul(self, &other.conj());
                let (nr, ni) = parts(&num);
                Ok(Number::complex(real_div(&nr, &den)?, real_div(&ni, &den)?))
            }
            r => {
                let (ar, ai) = parts(self);
                Ok(Number::complex(real_div(&ar, r)?, real_div(&ai, r)?))
            }
        }
    }

    /// `1 / self`.
    pub fn recip(&self) -> NumResult<Number> {
        Number::one().checked_div(self)
    }

    /// Total order on real numbers (exact across kinds); errors for complex.
    pub fn cmp_real(&self, other: &Number) -> NumResult<Ordering> {
        use Number::*;
        Ok(match (self, other) {
            (Complex(_), _) | (_, Complex(_)) => return Err(NumberError::NotReal),
            (Integer(a), Integer(b)) => a.cmp(b),
            (Integer(_) | Rational(_), Integer(_) | Rational(_)) => {
                as_rat(self).as_ref().cmp(as_rat(other).as_ref())
            }
            (Integer(_) | Rational(_), _) => {
                as_float(other, 53).cmp_rational(&as_rat(self)).reverse()
            }
            (_, Integer(_) | Rational(_)) => as_float(self, 53).cmp_rational(&as_rat(other)),
            _ => as_float(self, 53).cmp_value(&as_float(other, 53)),
        })
    }

    /// Smaller of two reals.
    pub fn min_real(&self, other: &Number) -> NumResult<Number> {
        Ok(if self.cmp_real(other)? == Ordering::Greater {
            other.clone()
        } else {
            self.clone()
        })
    }

    /// Larger of two reals.
    pub fn max_real(&self, other: &Number) -> NumResult<Number> {
        Ok(if self.cmp_real(other)? == Ordering::Less {
            other.clone()
        } else {
            self.clone()
        })
    }

    fn round_with(
        &self,
        rat: impl Fn(&BigRational) -> BigInt,
        float: impl Fn(&BigFloat) -> NumResult<BigInt>,
        f: impl Fn(f64) -> f64,
    ) -> NumResult<Number> {
        Ok(match self {
            Number::Integer(_) => self.clone(),
            Number::Rational(r) => Number::Integer(rat(r)),
            Number::Float(x) => Number::Integer(float(x)?),
            Number::F64(x) => Number::F64(f(*x)),
            Number::Complex(_) => return Err(NumberError::NotReal),
        })
    }

    /// Largest integer `<= self`. Exact (or `F64` for `F64` input).
    pub fn floor(&self) -> NumResult<Number> {
        self.round_with(|r| r.floor().to_integer(), BigFloat::floor, f64::floor)
    }

    /// Smallest integer `>= self`.
    pub fn ceil(&self) -> NumResult<Number> {
        self.round_with(|r| r.ceil().to_integer(), BigFloat::ceil, f64::ceil)
    }

    /// Nearest integer, halves rounded away from zero.
    pub fn round(&self) -> NumResult<Number> {
        self.round_with(|r| r.round().to_integer(), BigFloat::round, f64::round)
    }

    /// Integer part, rounding toward zero.
    pub fn trunc(&self) -> NumResult<Number> {
        self.round_with(|r| r.trunc().to_integer(), BigFloat::trunc, f64::trunc)
    }

    /// Absolute value (modulus for complex numbers, exact when possible,
    /// otherwise at the default precision or that of the parts).
    pub fn abs(&self) -> Number {
        self.abs_prec(Precision::DEFAULT)
    }

    /// Absolute value; the modulus of a complex number is computed at `prec`
    /// (or the precision of its float parts, if larger).
    pub fn abs_prec(&self, prec: Precision) -> Number {
        match self {
            Number::Integer(n) => Number::Integer(n.abs()),
            Number::Rational(r) => Number::Rational(r.abs()),
            Number::Float(f) => Number::Float(f.abs()),
            Number::F64(x) => Number::F64(x.abs()),
            Number::Complex(_) if has_f64_part(self) => {
                // hypot without f64 under/overflow
                f64_parts_to_big(self).abs_prec(prec).into_f64_kind()
            }
            Number::Complex(c) => {
                // Float squares cannot underflow (unbounded exponent).
                let sq = &c.re * &c.re + &c.im * &c.im;
                sq.sqrt(self.working_prec(prec))
                    .expect("square root of a non-negative real")
            }
        }
    }

    /// Sign: -1, 0 or 1 for reals (exact integer, `F64` for `F64`); `z/|z|`
    /// for complex numbers (at the default precision).
    pub fn sign(&self) -> NumResult<Number> {
        self.sign_prec(Precision::DEFAULT)
    }

    /// Sign, with `z/|z|` for complex numbers computed at `prec`.
    pub fn sign_prec(&self, prec: Precision) -> NumResult<Number> {
        match self.signum_i32() {
            Some(s) => Ok(match self {
                Number::F64(_) => Number::F64(s as f64),
                _ => Number::from(s),
            }),
            None => {
                let m = self.abs_prec(prec);
                if m.is_zero() {
                    return Err(NumberError::Domain("sign of a vanishing complex number"));
                }
                self.checked_div(&m)
            }
        }
    }

    /// Mathematical modulo: `a - b·floor(a/b)`, result has the sign of `b`
    /// (and `|result| < |b|`).
    pub fn modulo(&self, b: &Number) -> NumResult<Number> {
        if self.is_complex() || b.is_complex() {
            return Err(NumberError::NotReal);
        }
        if b.is_zero() {
            return Err(NumberError::DivisionByZero);
        }
        match (self, b) {
            (Number::Integer(x), Number::Integer(y)) => Ok(Number::Integer(x.mod_floor(y))),
            (Number::F64(_), _) | (_, Number::F64(_)) => {
                let (x, y) = (self.to_f64(), b.to_f64());
                if x.is_finite() && y.is_finite() {
                    return Ok(Number::F64(mod_f64(x, y)));
                }
                let r = f64_parts_to_big(self).modulo(&f64_parts_to_big(b))?;
                Ok(r.into_f64_kind())
            }
            (
                Number::Integer(_) | Number::Rational(_),
                Number::Integer(_) | Number::Rational(_),
            ) => {
                let q = self.checked_div(b)?.floor()?;
                Ok(self - &(b * &q))
            }
            _ => float_modulo(self, b),
        }
    }

    /// Greatest common divisor of exact rationals (non-negative). Integral
    /// floats are accepted too (`gcd(4.0, 6) = 2.0`, inexact result).
    /// `gcd(a/b, c/d) = gcd(a, c) / lcm(b, d)`.
    pub fn gcd(&self, other: &Number) -> NumResult<Number> {
        if let Some(r) = integral_float_op(self, other, |a, b| a.gcd(b)) {
            return r;
        }
        let (a, b) = exact_pair(self, other)?;
        let num = a.numer().gcd(b.numer());
        let den = a.denom().lcm(b.denom());
        Ok(Number::from_ratio(BigRational::new(num, den)))
    }

    /// Least common multiple of exact rationals (non-negative). Integral
    /// floats are accepted too (`lcm(4.0, 6) = 12.0`, inexact result).
    /// `lcm(a/b, c/d) = lcm(a, c) / gcd(b, d)`.
    pub fn lcm(&self, other: &Number) -> NumResult<Number> {
        if let Some(r) = integral_float_op(self, other, |a, b| a.lcm(b)) {
            return r;
        }
        let (a, b) = exact_pair(self, other)?;
        let num = a.numer().lcm(b.numer());
        let den = a.denom().gcd(b.denom());
        Ok(Number::from_ratio(BigRational::new(num, den)))
    }

    /// `x!`: exact for non-negative integers (including integral floats such
    /// as `5.0`), `Γ(x + 1)` numerically for other reals.
    /// Negative integers are a domain error.
    pub fn factorial(&self, prec: Precision) -> NumResult<Number> {
        match self.integer_value() {
            Ok(n) => super::theory::factorial(&n).map(Number::Integer),
            Err(NumberError::NotInteger) => (self + &Number::one()).gamma(prec),
            Err(e) => Err(e),
        }
    }

    /// Binomial coefficient `nCr(self, k)` for integers.
    pub fn ncr(&self, k: &Number) -> NumResult<Number> {
        super::theory::n_choose_k(&self.integer_value()?, &k.integer_value()?).map(Number::Integer)
    }

    /// Number of permutations `nPr(self, k)` for integers.
    pub fn npr(&self, k: &Number) -> NumResult<Number> {
        super::theory::n_permute_k(&self.integer_value()?, &k.integer_value()?).map(Number::Integer)
    }

    /// Integer square root `floor(√n)` for a non-negative integer.
    pub fn isqrt(&self) -> NumResult<Number> {
        let n = self.integer_value()?;
        if n.is_negative() {
            return Err(NumberError::Domain("isqrt of a negative number"));
        }
        Ok(Number::Integer(n.sqrt()))
    }

    /// Primality test for integers (negative numbers and 0, 1 are not prime).
    pub fn is_prime(&self) -> NumResult<bool> {
        super::theory::is_prime(&self.integer_value()?)
    }

    /// Prime factorization `[(p, k), ...]` of a non-zero integer
    /// (ascending; `-1` first for negative numbers).
    pub fn factorize(&self) -> NumResult<Vec<(BigInt, u32)>> {
        super::theory::factorize(&self.integer_value()?)
    }

    /// `self^n` for an exact integer exponent (exact for exact bases).
    pub(crate) fn powi(&self, n: &BigInt) -> NumResult<Number> {
        if n.is_zero() {
            return Ok(match self {
                Number::Float(f) => Number::Float(BigFloat::one(f.prec())),
                Number::F64(_) => Number::F64(1.0),
                _ => Number::one(),
            });
        }
        if self.is_zero() {
            return if n.is_negative() {
                Err(NumberError::DivisionByZero)
            } else {
                Ok(self.clone())
            };
        }
        let e = n.magnitude();
        let result = match self {
            Number::Integer(_) | Number::Rational(_) => {
                let r = as_rat(self);
                if r.numer().magnitude().is_one() && r.denom().is_one() {
                    // ±1
                    return Ok(if r.is_negative() && n.is_odd() {
                        Number::from(-1)
                    } else {
                        Number::one()
                    });
                }
                let bits = r.numer().bits().max(r.denom().bits()) as f64;
                let e_f = num_traits::ToPrimitive::to_f64(e).unwrap_or(f64::INFINITY);
                if bits * e_f > super::theory::MAX_EXACT_BITS as f64 {
                    return Err(NumberError::Limit);
                }
                let e = num_traits::ToPrimitive::to_u64(e).expect("bounded");
                let num = num_traits::pow::pow(r.numer().clone(), e as usize);
                let den = num_traits::pow::pow(r.denom().clone(), e as usize);
                Number::from_ratio(BigRational::new(num, den))
            }
            Number::Float(f) => return f.powi(n).map(Number::Float),
            Number::F64(x) => {
                let v = match num_traits::ToPrimitive::to_i32(n) {
                    Some(k) => x.powi(k),
                    None => x.powf(num_traits::ToPrimitive::to_f64(n).unwrap_or(f64::INFINITY)),
                };
                if v.is_finite() {
                    return Ok(Number::F64(v));
                }
                return f64_big(*x).powi(n).map(Number::Float);
            }
            Number::Complex(c) => {
                let bits = e.bits();
                if self.is_exact() {
                    let size = |x: &Number| {
                        let r = x.to_rational().expect("exact part");
                        r.numer().bits().max(r.denom().bits())
                    };
                    let part_bits = size(&c.re).max(size(&c.im)) + 1;
                    let e_f = num_traits::ToPrimitive::to_f64(e).unwrap_or(f64::INFINITY);
                    if part_bits as f64 * e_f > super::theory::MAX_EXACT_BITS as f64 {
                        return Err(NumberError::Limit);
                    }
                } else if bits > 20 {
                    // Polar form: avoids millions of multiplications and
                    // exponent blow-up; overflow is reported by `exp`.
                    let prec = self.working_prec(Precision::DEFAULT);
                    let n = Number::Integer(n.clone());
                    return (&n * &self.ln(prec)?).exp(prec);
                }
                let mut result = Number::one();
                let mut base = self.clone();
                for i in 0..bits {
                    super::check_interrupt()?;
                    if e.bit(i) {
                        result = &result * &base;
                    }
                    if i + 1 < bits {
                        base = &base * &base;
                    }
                }
                result
            }
        };
        if n.is_negative() {
            result.recip()
        } else {
            Ok(result)
        }
    }
}

/// `true` if the number is an `F64` or a complex number with an `F64` part.
fn has_f64_part(n: &Number) -> bool {
    match n {
        Number::F64(_) => true,
        Number::Complex(c) => has_f64_part(&c.re) || has_f64_part(&c.im),
        _ => false,
    }
}

/// Replaces `F64` parts by exact-valued 53-bit floats.
fn f64_parts_to_big(n: &Number) -> Number {
    match n {
        Number::F64(x) => Number::Float(f64_big(*x)),
        Number::Complex(c) => Number::complex(f64_parts_to_big(&c.re), f64_parts_to_big(&c.im)),
        other => other.clone(),
    }
}

/// Exponent range within which float modulo is computed exactly via rationals.
const EXACT_MOD_EXP: i64 = 1 << 14;

/// Modulo with at least one `Float` operand (no `F64`).
fn float_modulo(a: &Number, b: &Number) -> NumResult<Number> {
    let bits = |n: &Number| match n {
        Number::Float(f) => f.prec(),
        _ => 0,
    };
    let prec = bits(a).max(bits(b));
    let small = |n: &Number| match n {
        Number::Float(f) => f.is_zero() || f.exponent().abs() < EXACT_MOD_EXP,
        _ => true,
    };
    if small(a) && small(b) {
        // Floats are dyadic rationals: compute the exact remainder, round once.
        let exact = |n: &Number| match n {
            Number::Float(f) => f.to_rational(),
            other => other.to_rational().expect("exact real"),
        };
        let (x, y) = (exact(a), exact(b));
        let r = &x - &y * (&x / &y).floor();
        return Ok(Number::Float(BigFloat::from_ratio(
            r.numer(),
            r.denom(),
            prec,
        )));
    }
    let q = a.checked_div(b)?.floor()?;
    let mut r = a - &(b * &q);
    // Rounding in the quotient can leave r slightly outside [0, b): fix up.
    if !r.is_zero() && r.is_negative() != b.is_negative() {
        r = &r + b;
    }
    if r.abs().cmp_real(&b.abs())? != Ordering::Less {
        r = &r - b;
    }
    Ok(r)
}

/// Floored modulo of finite `f64`s (`y != 0`), shared by the `float64`
/// evaluator and the plot fast path so both agree bit for bit.
pub(crate) fn mod_f64(x: f64, y: f64) -> f64 {
    // `%` is exact in IEEE arithmetic; fix the sign afterwards.
    let mut r = x % y;
    if r != 0.0 && (r < 0.0) != (y < 0.0) {
        r += y;
    }
    if r.abs() >= y.abs() {
        r = 0.0;
    }
    r
}

/// `f` on the integer values of a real pair with at least one inexact
/// operand (`None` when both are exact or one is complex). Non-integral
/// floats are [`NumberError::NotInteger`]; the result is inexact (`F64` if
/// either operand is).
fn integral_float_op(
    a: &Number,
    b: &Number,
    f: fn(&BigInt, &BigInt) -> BigInt,
) -> Option<NumResult<Number>> {
    if (a.is_exact() && b.is_exact()) || a.is_complex() || b.is_complex() {
        return None;
    }
    let (x, y) = match (a.integer_value(), b.integer_value()) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
    };
    let r = Number::Integer(f(&x, &y));
    Some(Ok(
        if matches!(a, Number::F64(_)) || matches!(b, Number::F64(_)) {
            r.into_f64_kind()
        } else {
            r.to_float(a.working_prec(b.working_prec(Precision::from_bits(53))))
        },
    ))
}

fn exact_pair(a: &Number, b: &Number) -> NumResult<(BigRational, BigRational)> {
    match (a.to_rational(), b.to_rational()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ if a.is_complex() || b.is_complex() => Err(NumberError::NotReal),
        _ => Err(NumberError::NotInteger),
    }
}
