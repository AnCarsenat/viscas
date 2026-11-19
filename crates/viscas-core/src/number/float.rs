//! Arbitrary-precision binary floating point.

use std::cmp::Ordering;

use num_bigint::{BigInt, BigUint, Sign};
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

use super::{NumResult, NumberError};

/// Largest integer (in bits) that `floor`/`to_bigint` will materialize.
const MAX_INT_BITS: i64 = 1 << 26;

/// Largest binary exponent magnitude produced by `powi` and `exp`.
pub(crate) const MAX_EXP: i64 = 1 << 60;

/// An arbitrary-precision binary float: `(-1)^neg · man · 2^exp`.
///
/// The mantissa is rounded (half to even) to at most `prec` bits and kept odd
/// (trailing zero bits are moved into the exponent), so every value has a
/// unique representation. `prec` travels with the value; binary operations use
/// the larger precision of their operands.
#[derive(Clone, Debug)]
pub struct BigFloat {
    neg: bool,
    man: BigUint,
    exp: i64,
    prec: u64,
}

impl PartialEq for BigFloat {
    fn eq(&self, other: &Self) -> bool {
        self.neg == other.neg && self.man == other.man && self.exp == other.exp
    }
}

impl BigFloat {
    /// Rounds `(-1)^neg · man · 2^exp` to `prec` bits.
    pub(crate) fn from_parts(neg: bool, mut man: BigUint, mut exp: i64, prec: u64) -> Self {
        let prec = prec.max(2);
        if man.is_zero() {
            return Self::zero(prec);
        }
        let bits = man.bits();
        if bits > prec {
            let shift = bits - prec;
            let half = man.bit(shift - 1);
            let sticky = man.trailing_zeros().is_some_and(|tz| tz < shift - 1);
            man >>= shift;
            exp += shift as i64;
            if half && (sticky || man.bit(0)) {
                man += 1u32;
            }
        }
        let tz = man.trailing_zeros().unwrap_or(0);
        if tz > 0 {
            man >>= tz;
            exp += tz as i64;
        }
        BigFloat {
            neg,
            man,
            exp,
            prec,
        }
    }

    /// Builds from a signed integer mantissa.
    pub(crate) fn from_signed(man: BigInt, exp: i64, prec: u64) -> Self {
        let (sign, mag) = man.into_parts();
        Self::from_parts(sign == Sign::Minus, mag, exp, prec)
    }

    /// Zero with the given precision (in bits).
    pub fn zero(prec: u64) -> Self {
        BigFloat {
            neg: false,
            man: BigUint::zero(),
            exp: 0,
            prec: prec.max(2),
        }
    }

    /// One with the given precision (in bits).
    pub fn one(prec: u64) -> Self {
        Self::from_parts(false, BigUint::one(), 0, prec)
    }

    /// `n` rounded to `prec` bits.
    pub fn from_bigint(n: &BigInt, prec: u64) -> Self {
        Self::from_signed(n.clone(), 0, prec)
    }

    /// Small integer.
    pub fn from_i64(n: i64, prec: u64) -> Self {
        Self::from_signed(BigInt::from(n), 0, prec)
    }

    /// `num / den` correctly rounded to `prec` bits. `den` must be non-zero.
    pub fn from_ratio(num: &BigInt, den: &BigInt, prec: u64) -> Self {
        let a = Self::from_signed(num.clone(), 0, u64::MAX);
        let b = Self::from_signed(den.clone(), 0, u64::MAX);
        a.div_prec(&b, prec).expect("non-zero denominator")
    }

    /// Exact conversion of a finite `f64` (then rounded to `prec` bits).
    pub fn from_f64(x: f64, prec: u64) -> Option<Self> {
        if !x.is_finite() {
            return None;
        }
        if x == 0.0 {
            return Some(Self::zero(prec));
        }
        let bits = x.to_bits();
        let neg = bits >> 63 == 1;
        let biased = ((bits >> 52) & 0x7ff) as i64;
        let frac = bits & ((1u64 << 52) - 1);
        let (man, exp) = if biased == 0 {
            (frac, -1074)
        } else {
            (frac | (1u64 << 52), biased - 1075)
        };
        Some(Self::from_parts(neg, BigUint::from(man), exp, prec))
    }

    /// Precision in bits.
    pub fn prec(&self) -> u64 {
        self.prec
    }

    /// Same value rounded to a new precision.
    pub fn with_prec(&self, prec: u64) -> Self {
        Self::from_parts(self.neg, self.man.clone(), self.exp, prec)
    }

    /// `true` for zero.
    pub fn is_zero(&self) -> bool {
        self.man.is_zero()
    }

    /// `true` for values `< 0`.
    pub fn is_negative(&self) -> bool {
        self.neg && !self.is_zero()
    }

    /// `true` if the value is an integer.
    pub fn is_integral(&self) -> bool {
        self.is_zero() || self.exp >= 0
    }

    /// Binary magnitude: `2^(m-1) <= |x| < 2^m`. Zero gives `i64::MIN`.
    pub fn magnitude(&self) -> i64 {
        if self.is_zero() {
            i64::MIN
        } else {
            self.man.bits() as i64 + self.exp
        }
    }

    /// Binary exponent of the least significant mantissa bit.
    pub(crate) fn exponent(&self) -> i64 {
        self.exp
    }

    /// Signed mantissa.
    pub(crate) fn signed_mantissa(&self) -> BigInt {
        BigInt::from_biguint(
            if self.neg { Sign::Minus } else { Sign::Plus },
            self.man.clone(),
        )
    }

    /// `-self`.
    pub fn neg(&self) -> Self {
        let mut r = self.clone();
        if !r.is_zero() {
            r.neg = !r.neg;
        }
        r
    }

    /// `|self|`.
    pub fn abs(&self) -> Self {
        let mut r = self.clone();
        r.neg = false;
        r
    }

    /// `self · 2^k` (exact).
    pub fn mul_pow2(&self, k: i64) -> Self {
        let mut r = self.clone();
        if !r.is_zero() {
            r.exp += k;
        }
        r
    }

    /// Exact value as a rational. Panics on absurdly large exponents; callers
    /// guard with [`BigFloat::magnitude`].
    pub fn to_rational(&self) -> BigRational {
        let m = self.signed_mantissa();
        if self.exp >= 0 {
            BigRational::from_integer(m << self.exp as usize)
        } else {
            BigRational::new(m, BigInt::one() << (-self.exp) as usize)
        }
    }

    /// Nearest `f64` (infinite if out of range).
    pub fn to_f64(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        let r = self.with_prec(53);
        let m = r.man.to_u64().expect("53-bit mantissa") as f64;
        let v = ldexp(m, r.exp);
        if r.neg { -v } else { v }
    }

    /// `self + other` with precision `max(prec)`.
    pub fn add(&self, other: &Self) -> Self {
        self.add_prec(other, self.prec.max(other.prec))
    }

    /// `self - other` with precision `max(prec)`.
    pub fn sub(&self, other: &Self) -> Self {
        self.add_prec(&other.neg(), self.prec.max(other.prec))
    }

    /// `self + other` rounded to `prec` bits.
    pub fn add_prec(&self, other: &Self, prec: u64) -> Self {
        if self.is_zero() {
            return other.with_prec(prec);
        }
        if other.is_zero() {
            return self.with_prec(prec);
        }
        let (ma, mb) = (self.magnitude(), other.magnitude());
        let gap = iprec(prec) + 3;
        if ma > mb.saturating_add(gap) {
            return self.with_prec(prec);
        }
        if mb > ma.saturating_add(gap) {
            return other.with_prec(prec);
        }
        let e = self.exp.min(other.exp);
        let a = self.signed_mantissa() << (self.exp - e) as usize;
        let b = other.signed_mantissa() << (other.exp - e) as usize;
        Self::from_signed(a + b, e, prec)
    }

    /// `self · other` with precision `max(prec)`.
    pub fn mul(&self, other: &Self) -> Self {
        self.mul_prec(other, self.prec.max(other.prec))
    }

    /// `self · other` rounded to `prec` bits.
    pub fn mul_prec(&self, other: &Self, prec: u64) -> Self {
        Self::from_parts(
            self.neg != other.neg,
            &self.man * &other.man,
            self.exp + other.exp,
            prec,
        )
    }

    /// `self / other` with precision `max(prec)`; `None` if `other` is zero.
    pub fn div(&self, other: &Self) -> Option<Self> {
        self.div_prec(other, self.prec.max(other.prec))
    }

    /// `self / other` correctly rounded to `prec` bits; `None` if `other` is zero.
    pub fn div_prec(&self, other: &Self, prec: u64) -> Option<Self> {
        if other.is_zero() {
            return None;
        }
        if self.is_zero() {
            return Some(Self::zero(prec));
        }
        let prec = prec.max(2);
        let want = iprec(prec) + 3 + other.man.bits() as i64 - self.man.bits() as i64;
        let shift = want.max(0) as usize;
        let num = &self.man << shift;
        let (mut q, r) = num.div_rem(&other.man);
        let mut exp = self.exp - shift as i64 - other.exp;
        if !r.is_zero() {
            q = (q << 1usize) | BigUint::one();
            exp -= 1;
        }
        Some(Self::from_parts(self.neg != other.neg, q, exp, prec))
    }

    /// Square root, correctly rounded; `None` for negative inputs.
    pub fn sqrt(&self) -> Option<Self> {
        self.sqrt_prec(self.prec)
    }

    /// Square root rounded to `prec` bits; `None` for negative inputs.
    pub fn sqrt_prec(&self, prec: u64) -> Option<Self> {
        if self.is_zero() {
            return Some(Self::zero(prec));
        }
        if self.neg {
            return None;
        }
        let want = 2 * (iprec(prec) + 3) - self.man.bits() as i64;
        let mut shift = want.max(0);
        if (self.exp - shift) % 2 != 0 {
            shift += 1;
        }
        let n = &self.man << shift as usize;
        let mut r = n.sqrt();
        let mut exp = (self.exp - shift) / 2;
        if &r * &r != n {
            r = (r << 1usize) | BigUint::one();
            exp -= 1;
        }
        Some(Self::from_parts(false, r, exp, prec))
    }

    /// `self^n` for an integer exponent, rounded to `self.prec`.
    pub fn powi(&self, n: &BigInt) -> NumResult<Self> {
        let prec = self.prec;
        if n.is_zero() {
            return Ok(Self::one(prec));
        }
        if self.is_zero() {
            return if n.is_negative() {
                Err(NumberError::DivisionByZero)
            } else {
                Ok(Self::zero(prec))
            };
        }
        let mag = self.magnitude();
        let n_f = n.to_f64().unwrap_or(f64::INFINITY);
        let est = (mag.abs() as f64 + 1.0) * n_f.abs();
        if self.man.is_one() && self.exp == 0 {
            // |x| = 1
            let odd = n.is_odd();
            return Ok(if self.neg && odd {
                Self::one(prec).neg()
            } else {
                Self::one(prec)
            });
        }
        if est > MAX_EXP as f64 {
            return Err(NumberError::Overflow);
        }
        let mag_n = n.magnitude();
        let wp = prec + mag_n.bits() + 10;
        let mut result = Self::one(wp);
        let mut base = self.with_prec(wp);
        let bits = mag_n.bits();
        for i in 0..bits {
            if mag_n.bit(i) {
                result = result.mul_prec(&base, wp);
            }
            if i + 1 < bits {
                base = base.mul_prec(&base, wp);
            }
        }
        if n.is_negative() {
            result = Self::one(wp)
                .div_prec(&result, wp)
                .ok_or(NumberError::DivisionByZero)?;
        }
        Ok(result.with_prec(prec))
    }

    /// Exact comparison.
    pub fn cmp_value(&self, other: &Self) -> Ordering {
        let sa = sign(self);
        let sb = sign(other);
        if sa != sb {
            return sa.cmp(&sb);
        }
        if sa == 0 {
            return Ordering::Equal;
        }
        let abs_order = self.cmp_abs(other);
        if sa > 0 {
            abs_order
        } else {
            abs_order.reverse()
        }
    }

    /// Exact comparison of absolute values (both non-zero).
    fn cmp_abs(&self, other: &Self) -> Ordering {
        let (ma, mb) = (self.magnitude(), other.magnitude());
        if ma != mb {
            return ma.cmp(&mb);
        }
        let e = self.exp.min(other.exp);
        let a = &self.man << (self.exp - e) as usize;
        let b = &other.man << (other.exp - e) as usize;
        a.cmp(&b)
    }

    /// Exact comparison with a rational.
    pub fn cmp_rational(&self, r: &BigRational) -> Ordering {
        if self.exp.abs() < MAX_INT_BITS {
            return self.to_rational().cmp(r);
        }
        // Astronomically large or small float: magnitudes decide.
        let sr = sign_rat(r);
        let sa = sign(self);
        if sa != sr || sa == 0 {
            return sa.cmp(&sr);
        }
        let mr = r.numer().bits() as i64 - r.denom().bits() as i64;
        let order = self.magnitude().cmp(&mr);
        if sa > 0 { order } else { order.reverse() }
    }

    /// Largest integer `<= self`. Errors if the integer would be enormous.
    pub fn floor(&self) -> NumResult<BigInt> {
        if self.exp >= 0 {
            if self.magnitude() > MAX_INT_BITS {
                return Err(NumberError::Overflow);
            }
            return Ok(self.signed_mantissa() << self.exp as usize);
        }
        if self.magnitude() <= 0 {
            return Ok(if self.neg {
                -BigInt::one()
            } else {
                BigInt::zero()
            });
        }
        let d = BigInt::one() << (-self.exp) as usize;
        Ok(self.signed_mantissa().div_floor(&d))
    }

    /// Smallest integer `>= self`.
    pub fn ceil(&self) -> NumResult<BigInt> {
        Ok(-self.neg().floor()?)
    }

    /// Nearest integer, halves rounded away from zero.
    pub fn round(&self) -> NumResult<BigInt> {
        let half = Self::from_parts(false, BigUint::one(), -1, 2);
        let r = self.abs().add_prec(&half, u64::MAX).floor()?;
        Ok(if self.neg { -r } else { r })
    }

    /// Integer part (rounded toward zero).
    pub fn trunc(&self) -> NumResult<BigInt> {
        if self.neg { self.ceil() } else { self.floor() }
    }

    /// `round(self · 2^wp)` as a fixed-point integer (truncated toward -inf).
    pub(crate) fn to_fixed(&self, wp: u64) -> BigInt {
        let shift = self.exp + wp as i64;
        let m = self.signed_mantissa();
        if shift >= 0 {
            m << shift as usize
        } else if -shift > self.man.bits() as i64 + 1 {
            if self.neg {
                -BigInt::one()
            } else {
                BigInt::zero()
            }
        } else {
            m >> (-shift) as usize
        }
    }

    /// Fixed-point integer `v / 2^wp` rounded to `prec` bits.
    pub(crate) fn from_fixed(v: BigInt, wp: u64, prec: u64) -> Self {
        Self::from_signed(v, -(wp as i64), prec)
    }
}

/// Precision as a signed bit count, capped so "unbounded" precisions stay sane.
fn iprec(prec: u64) -> i64 {
    prec.min(1 << 40) as i64
}

fn sign(x: &BigFloat) -> i32 {
    if x.is_zero() {
        0
    } else if x.neg {
        -1
    } else {
        1
    }
}

fn sign_rat(r: &BigRational) -> i32 {
    if r.is_zero() {
        0
    } else if r.is_negative() {
        -1
    } else {
        1
    }
}

/// `x · 2^e` for `f64`, stepping to avoid intermediate overflow.
fn ldexp(mut x: f64, mut e: i64) -> f64 {
    let up = 2f64.powi(1000);
    let down = 2f64.powi(-1000);
    while e > 1000 && x.is_finite() {
        x *= up;
        e -= 1000;
    }
    while e < -1000 && x != 0.0 {
        x *= down;
        e += 1000;
    }
    if e > 1000 {
        return x;
    }
    if e < -1000 {
        return 0.0;
    }
    // Split to keep the factor in normal range.
    let half = e / 2;
    x * 2f64.powi(half as i32) * 2f64.powi((e - half) as i32)
}
