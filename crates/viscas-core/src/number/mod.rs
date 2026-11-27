//! Numeric tower: exact integers and rationals, arbitrary-precision floats,
//! a fast `f64` path for plotting, and complex numbers built on top of them.
//!
//! # Overview
//!
//! [`Number`] is the value type used by the evaluator and the CAS:
//!
//! | Variant              | Exact | Notes                                             |
//! |----------------------|-------|---------------------------------------------------|
//! | [`Number::Integer`]  | yes   | arbitrary size ([`BigInt`])                       |
//! | [`Number::Rational`] | yes   | always normalized, denominator > 1                |
//! | [`Number::Float`]    | no    | binary float, configurable precision ([`BigFloat`]) |
//! | [`Number::F64`]      | no    | hardware double, always finite                    |
//! | [`Number::Complex`]  | mixed | real and imaginary parts are real `Number`s       |
//!
//! Arithmetic promotes automatically along `Integer ⊂ Rational ⊂ Float ⊂ F64`
//! (the "least precise" kind wins), so exact inputs give exact results
//! (`1/3 + 1/6 = 1/2`, `7 / 2 = 7/2`). Floating values only appear when a
//! literal or an operation is inherently inexact (`sin`, `ln`, irrational
//! powers, ...). Values are always kept in canonical form: a rational with
//! denominator one becomes an integer and a complex number whose imaginary part
//! is zero (exact or not) becomes real.
//!
//! `F64` is contagious on purpose: it is the plotting fast-path type, and the
//! evaluator only produces it in plot mode, where it is never mixed with exact
//! values it would degrade. An `F64` is always finite: operations whose `f64`
//! result would overflow fall back to a 53-bit [`BigFloat`] instead.
//!
//! Operations that can fail (division by zero, domain errors, comparing complex
//! numbers, ...) return [`NumberError`]. Operations whose result may not be
//! exactly representable come in two flavours: an `*_exact` variant returning
//! `Option` (so the CAS can keep the expression symbolic, e.g. `2^(1/2)`) and a
//! numeric variant taking a [`Precision`].
//!
//! # Cost limits and cancellation
//!
//! Requests that would be unreasonably expensive (astronomically large exact
//! results, primality of huge numbers, factorizations that exhaust their
//! iteration budget, ...) fail with [`NumberError::Limit`], which is distinct
//! from [`NumberError::Overflow`] (a value that cannot be represented at all).
//! Long-running loops (series, factorization, primality, big products) poll a
//! per-thread interrupt flag installed with [`install_interrupt`] and return
//! [`NumberError::Interrupted`] once it is set, so a UI worker can cancel them.
//!
//! # Equality and hashing
//!
//! `PartialEq` compares numeric values across kinds (`1 == 1.0 == 2/2`).
//! `Number` deliberately does not implement `Hash`; any future implementation
//! must hash the canonical numeric value so that it agrees with `PartialEq`.
//!
//! # Crate choice
//!
//! The big-number backend is the `num` family (`num-bigint`, `num-rational`,
//! `num-integer`, `num-traits`): pure Rust, no C dependencies, very widely used
//! and stable, with the integer primitives we need (`sqrt`, `nth_root`,
//! `modpow`, gcd). `dashu` was considered, but its float type has no
//! trigonometric functions either, so the elementary functions would have to be
//! written by hand anyway; with `num` we implement a small, fully controlled
//! binary float ([`BigFloat`], mantissa × 2^exponent with round-half-even) and
//! mpmath-style fixed-point series for every elementary function, which keeps
//! precision handling uniform across all functions.

mod arith;
pub(crate) use arith::mod_f64;
mod complex;
mod elementary;
mod float;
mod format;
mod functions;
mod gamma;
pub(crate) use gamma::gamma_f64;
mod interrupt;
mod parse;
mod theory;

#[cfg(test)]
mod tests;

use std::fmt;

pub use num_bigint::BigInt;
pub use num_rational::BigRational;

pub use float::BigFloat;
pub use interrupt::{InterruptGuard, check_interrupt, install_interrupt};
pub use parse::parse_literal;
pub use theory::{factorial, factorize, is_prime, n_choose_k, n_permute_k};

use num_traits::{One, Signed, ToPrimitive, Zero};

/// Errors produced by numeric operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NumberError {
    /// Division (or modulo, or negative power) by zero.
    DivisionByZero,
    /// Argument outside the function's domain, e.g. `asin(2)` or `ln(0)`.
    Domain(&'static str),
    /// A real number was required (ordering, `floor`, ...) but a complex one was given.
    NotReal,
    /// An exact integer was required (`gcd`, `factorial`, ...).
    NotInteger,
    /// Result too large (or too small) to represent at all.
    Overflow,
    /// Refused because it would be too expensive to compute (time or memory).
    Limit,
    /// Cancelled through the interrupt flag (see [`install_interrupt`]).
    Interrupted,
    /// Malformed numeric literal.
    Parse(String),
}

impl fmt::Display for NumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberError::DivisionByZero => write!(f, "division by zero"),
            NumberError::Domain(what) => write!(f, "argument out of domain: {what}"),
            NumberError::NotReal => write!(f, "expected a real number"),
            NumberError::NotInteger => write!(f, "expected an integer"),
            NumberError::Overflow => write!(f, "number too large"),
            NumberError::Limit => write!(f, "computation too expensive"),
            NumberError::Interrupted => write!(f, "computation interrupted"),
            NumberError::Parse(text) => write!(f, "invalid number literal: {text}"),
        }
    }
}

impl std::error::Error for NumberError {}

/// Result alias for numeric operations.
pub type NumResult<T> = Result<T, NumberError>;

/// Working precision for arbitrary-precision floats.
///
/// Stored in bits so that converting from a float's precision and back is
/// lossless; constructed from (and reported as) significant decimal digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Precision {
    bits: u64,
}

impl Precision {
    /// Default precision: 50 significant digits.
    pub const DEFAULT: Precision = Precision { bits: 167 };

    /// Largest supported precision, in decimal digits.
    pub const MAX_DIGITS: u32 = 100_000;

    /// Precision of `digits` significant decimal digits, clamped to
    /// `1..=MAX_DIGITS`.
    pub fn digits(digits: u32) -> Self {
        let d = digits.clamp(1, Self::MAX_DIGITS);
        Precision {
            bits: ((d as f64) * std::f64::consts::LOG2_10).ceil() as u64,
        }
    }

    /// Precision of exactly `bits` binary digits, clamped to the supported range.
    pub fn from_bits(bits: u64) -> Self {
        let max = Precision::digits(Self::MAX_DIGITS).bits;
        Precision {
            bits: bits.clamp(4, max),
        }
    }

    /// Number of significant decimal digits (rounded down, at least 1).
    pub fn get(self) -> u32 {
        (((self.bits as f64) * std::f64::consts::LOG10_2).floor() as u32).max(1)
    }

    /// Number of mantissa bits.
    pub fn bits(self) -> u64 {
        self.bits
    }
}

impl Default for Precision {
    fn default() -> Self {
        Precision::DEFAULT
    }
}

/// A complex number whose parts are real [`Number`]s.
///
/// Invariant (maintained by [`Number::complex`]): both parts are real and the
/// imaginary part is not zero.
#[derive(Clone, Debug)]
pub struct Complex {
    re: Number,
    im: Number,
}

impl Complex {
    /// Real part.
    pub fn re(&self) -> &Number {
        &self.re
    }

    /// Imaginary part.
    pub fn im(&self) -> &Number {
        &self.im
    }
}

/// A number of the Viscas numeric tower. See the [module docs](self).
#[derive(Clone, Debug)]
pub enum Number {
    /// Exact integer.
    Integer(BigInt),
    /// Exact rational, normalized with denominator > 1.
    Rational(BigRational),
    /// Arbitrary-precision binary float.
    Float(BigFloat),
    /// Hardware double (fast path for plotting); always finite.
    F64(f64),
    /// Complex number.
    Complex(Box<Complex>),
}

impl Number {
    /// The exact integer zero.
    pub fn zero() -> Number {
        Number::Integer(BigInt::zero())
    }

    /// The exact integer one.
    pub fn one() -> Number {
        Number::Integer(BigInt::one())
    }

    /// The imaginary unit `i`.
    pub fn i() -> Number {
        Number::Complex(Box::new(Complex {
            re: Number::zero(),
            im: Number::one(),
        }))
    }

    /// Exact integer from anything convertible to [`BigInt`].
    pub fn integer(n: impl Into<BigInt>) -> Number {
        Number::Integer(n.into())
    }

    /// Exact rational `num / den`, normalized. Errors if `den` is zero.
    pub fn rational(num: impl Into<BigInt>, den: impl Into<BigInt>) -> NumResult<Number> {
        let den = den.into();
        if den.is_zero() {
            return Err(NumberError::DivisionByZero);
        }
        Ok(Number::from_ratio(BigRational::new(num.into(), den)))
    }

    /// Normalizes a [`BigRational`] into an integer when its denominator is one.
    pub fn from_ratio(r: BigRational) -> Number {
        if r.denom().is_one() {
            Number::Integer(r.to_integer())
        } else {
            Number::Rational(r)
        }
    }

    /// Hardware float. Errors with [`NumberError::Domain`] if `x` is not finite.
    pub fn f64(x: f64) -> NumResult<Number> {
        if x.is_finite() {
            Ok(Number::F64(x))
        } else {
            Err(NumberError::Domain("non-finite float"))
        }
    }

    /// Builds `re + im·i`. Parts may themselves be complex (the result is then
    /// computed arithmetically). Collapses to a real number if the imaginary
    /// part is zero (exact or inexact).
    pub fn complex(re: Number, im: Number) -> Number {
        if re.is_complex() || im.is_complex() {
            return re + im * Number::i();
        }
        if im.is_zero() {
            re
        } else {
            Number::Complex(Box::new(Complex { re, im }))
        }
    }

    /// `self · i`.
    pub fn imaginary(self) -> Number {
        self * Number::i()
    }

    /// `true` for exact kinds (integers, rationals, complex with exact parts).
    pub fn is_exact(&self) -> bool {
        match self {
            Number::Integer(_) | Number::Rational(_) => true,
            Number::Float(_) | Number::F64(_) => false,
            Number::Complex(c) => c.re.is_exact() && c.im.is_exact(),
        }
    }

    /// `true` unless this is a complex number.
    pub fn is_real(&self) -> bool {
        !self.is_complex()
    }

    /// `true` for the complex variant.
    pub fn is_complex(&self) -> bool {
        matches!(self, Number::Complex(_))
    }

    /// `true` if the value is zero (of any kind).
    pub fn is_zero(&self) -> bool {
        match self {
            Number::Integer(n) => n.is_zero(),
            Number::Rational(r) => r.is_zero(),
            Number::Float(f) => f.is_zero(),
            Number::F64(x) => *x == 0.0,
            Number::Complex(_) => false,
        }
    }

    /// `true` only for an exact zero.
    pub fn is_exact_zero(&self) -> bool {
        matches!(self, Number::Integer(n) if n.is_zero())
    }

    /// `true` if the value is an exact one.
    pub fn is_exact_one(&self) -> bool {
        matches!(self, Number::Integer(n) if n.is_one())
    }

    /// `true` for exact integers.
    pub fn is_integer(&self) -> bool {
        matches!(self, Number::Integer(_))
    }

    /// `true` for real values with integral value (of any real kind).
    pub fn is_integral(&self) -> bool {
        match self {
            Number::Integer(_) => true,
            Number::Rational(_) | Number::Complex(_) => false,
            Number::Float(f) => f.is_integral(),
            Number::F64(x) => x.fract() == 0.0,
        }
    }

    /// The exact integer, if this is one.
    pub fn as_integer(&self) -> Option<&BigInt> {
        match self {
            Number::Integer(n) => Some(n),
            _ => None,
        }
    }

    /// The exact value as a rational, if this is an exact real.
    pub fn to_rational(&self) -> Option<BigRational> {
        match self {
            Number::Integer(n) => Some(BigRational::from_integer(n.clone())),
            Number::Rational(r) => Some(r.clone()),
            _ => None,
        }
    }

    /// Sign of a real number as -1, 0 or 1; `None` for complex numbers.
    pub fn signum_i32(&self) -> Option<i32> {
        let s = match self {
            Number::Integer(n) => sign_of(n.is_zero(), n.is_negative()),
            Number::Rational(r) => sign_of(r.is_zero(), r.is_negative()),
            Number::Float(f) => sign_of(f.is_zero(), f.is_negative()),
            Number::F64(x) => sign_of(*x == 0.0, *x < 0.0),
            Number::Complex(_) => return None,
        };
        Some(s)
    }

    /// `true` for real values `< 0`.
    pub fn is_negative(&self) -> bool {
        self.signum_i32() == Some(-1)
    }

    /// Real part (the number itself if real).
    pub fn re(&self) -> Number {
        match self {
            Number::Complex(c) => c.re.clone(),
            other => other.clone(),
        }
    }

    /// Imaginary part (exact zero if real).
    pub fn im(&self) -> Number {
        match self {
            Number::Complex(c) => c.im.clone(),
            _ => Number::zero(),
        }
    }

    /// Complex conjugate.
    pub fn conj(&self) -> Number {
        match self {
            Number::Complex(c) => Number::complex(c.re.clone(), -c.im.clone()),
            other => other.clone(),
        }
    }

    /// Converts to an arbitrary-precision float. Exact values are rounded to
    /// `prec`; floats keep their own precision. `None` for complex numbers.
    pub fn to_bigfloat(&self, prec: Precision) -> Option<BigFloat> {
        let bits = prec.bits();
        match self {
            Number::Integer(n) => Some(BigFloat::from_bigint(n, bits)),
            Number::Rational(r) => Some(BigFloat::from_ratio(r.numer(), r.denom(), bits)),
            Number::Float(f) => Some(f.clone()),
            Number::F64(x) => BigFloat::from_f64(*x, 53),
            Number::Complex(_) => None,
        }
    }

    /// Converts every exact part to a [`Number::Float`] of precision `prec`.
    /// Inexact values are returned unchanged.
    pub fn to_float(&self, prec: Precision) -> Number {
        match self {
            Number::Integer(_) | Number::Rational(_) => {
                Number::Float(self.to_bigfloat(prec).expect("real"))
            }
            Number::Float(_) | Number::F64(_) => self.clone(),
            Number::Complex(c) => Number::complex(c.re.to_float(prec), c.im.to_float(prec)),
        }
    }

    /// Nearest `f64` of a real number; `NaN` for non-real complex numbers.
    pub fn to_f64(&self) -> f64 {
        match self {
            Number::Integer(n) => n.to_f64().unwrap_or(f64::NAN),
            Number::Rational(r) => BigFloat::from_ratio(r.numer(), r.denom(), 64).to_f64(),
            Number::Float(f) => f.to_f64(),
            Number::F64(x) => *x,
            Number::Complex(_) => f64::NAN,
        }
    }

    /// `(re, im)` as `f64`s.
    pub fn to_complex_f64(&self) -> (f64, f64) {
        match self {
            Number::Complex(c) => (c.re.to_f64(), c.im.to_f64()),
            other => (other.to_f64(), 0.0),
        }
    }

    /// Converts a value computed for an `F64` operation back to the `F64` kind:
    /// `F64` if the value is finite as an `f64`, otherwise a 53-bit
    /// [`Number::Float`] (keeps the "F64 is always finite" invariant).
    pub(crate) fn into_f64_kind(self) -> Number {
        match self {
            Number::Complex(c) => Number::complex(c.re.into_f64_kind(), c.im.into_f64_kind()),
            Number::F64(_) => self,
            other => {
                let x = other.to_f64();
                // Out of range (inf) or underflowed to zero: keep a 53-bit float.
                if x.is_finite() && (x != 0.0 || other.is_zero()) {
                    Number::F64(x)
                } else {
                    let big = other.to_bigfloat(Precision::from_bits(53)).expect("real");
                    Number::Float(big.with_prec(53))
                }
            }
        }
    }

    /// The integer value of an exact integer or of an exactly integral
    /// `Float`/`F64` (`5.0` is accepted where an integer is required).
    pub fn integer_value(&self) -> NumResult<BigInt> {
        match self {
            Number::Integer(n) => Ok(n.clone()),
            Number::Float(f) if f.is_integral() => f.floor(),
            Number::F64(x) if x.fract() == 0.0 => {
                Ok(BigFloat::from_f64(*x, 64).expect("finite").floor()?)
            }
            Number::Complex(_) => Err(NumberError::NotReal),
            _ => Err(NumberError::NotInteger),
        }
    }

    /// The precision of floating parts, if any (largest one), else `default`.
    pub(crate) fn working_prec(&self, default: Precision) -> Precision {
        match self {
            Number::Float(f) => Precision::from_bits(f.prec()).max(default),
            Number::Complex(c) => c.re.working_prec(c.im.working_prec(default)),
            _ => default,
        }
    }
}

fn sign_of(zero: bool, neg: bool) -> i32 {
    if zero {
        0
    } else if neg {
        -1
    } else {
        1
    }
}

impl From<i64> for Number {
    fn from(n: i64) -> Self {
        Number::Integer(BigInt::from(n))
    }
}

impl From<i32> for Number {
    fn from(n: i32) -> Self {
        Number::Integer(BigInt::from(n))
    }
}

impl From<u64> for Number {
    fn from(n: u64) -> Self {
        Number::Integer(BigInt::from(n))
    }
}

impl From<BigInt> for Number {
    fn from(n: BigInt) -> Self {
        Number::Integer(n)
    }
}

impl From<BigRational> for Number {
    fn from(r: BigRational) -> Self {
        Number::from_ratio(r)
    }
}

impl From<BigFloat> for Number {
    fn from(f: BigFloat) -> Self {
        Number::Float(f)
    }
}

impl std::str::FromStr for Number {
    type Err = NumberError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_literal(s)
    }
}
