//! Elementary functions and powers on [`Number`].
//!
//! Rules:
//! * `F64` inputs are computed with `f64` (falling back to a 53-bit
//!   [`BigFloat`] if the `f64` result overflows).
//! * `Float` inputs keep their own precision.
//! * Exact inputs are evaluated at the requested [`Precision`], except for the
//!   trivial exact cases (`sin 0 = 0`, `exp 0 = 1`, `ln 1 = 0`, perfect squares
//!   under `sqrt`, exact powers of the base under `log`, ...).
//! * `sqrt`, `exp`, `ln`, `log*`, `pow`, `sin`, `cos` and `tan` accept complex
//!   arguments (principal branch); the other functions return
//!   [`NumberError::NotReal`] for complex input and [`NumberError::Domain`] for
//!   real arguments outside their real domain.

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Signed, ToPrimitive, Zero};

use super::{BigFloat, NumResult, Number, NumberError, Precision, complex};

type BigFn = fn(&BigFloat) -> NumResult<BigFloat>;

impl Number {
    /// π as a float of precision `prec`.
    pub fn pi(prec: Precision) -> NumResult<Number> {
        BigFloat::pi(prec.bits()).map(Number::Float)
    }

    /// Euler's number e as a float of precision `prec`.
    pub fn e(prec: Precision) -> NumResult<Number> {
        BigFloat::e(prec.bits()).map(Number::Float)
    }

    /// Applies a real function: `big` for exact/`Float`, `small` for `F64`.
    fn real_map(&self, prec: Precision, big: BigFn, small: fn(f64) -> f64) -> NumResult<Number> {
        match self {
            Number::F64(x) => {
                let y = small(*x);
                if y.is_finite() {
                    Ok(Number::F64(y))
                } else {
                    let b = BigFloat::from_f64(*x, 53).expect("finite");
                    big(&b).map(Number::Float)
                }
            }
            Number::Complex(_) => Err(NumberError::NotReal),
            Number::Float(f) => big(f).map(Number::Float),
            exact => {
                let bits = prec.bits();
                let x = exact.exact_to_bigfloat(bits + exact.near_one_extra(bits));
                Ok(Number::Float(big(&x)?.with_prec(bits)))
            }
        }
    }

    /// Converts an exact real to a float with `bits` of precision.
    fn exact_to_bigfloat(&self, bits: u64) -> BigFloat {
        let r = self.to_rational().expect("exact real");
        BigFloat::from_ratio(r.numer(), r.denom(), bits)
    }

    /// Extra input bits for exact values close to ±1, where functions such as
    /// `ln`, `acos` or `atanh` are ill-conditioned: rounding `1 + 10^-40` to
    /// the working precision would otherwise destroy most significant digits
    /// of `ln(1 + 10^-40)`.
    fn near_one_extra(&self, bits: u64) -> u64 {
        let Some(r) = self.to_rational() else {
            return 0;
        };
        let d = r.abs() - BigRational::from_integer(1.into());
        if d.is_zero() {
            return 0;
        }
        let lost = d.denom().bits() as i64 - d.numer().bits() as i64;
        (lost.max(0) as u64).min(4 * bits)
    }

    fn require_real(&self) -> NumResult<()> {
        if self.is_complex() {
            Err(NumberError::NotReal)
        } else {
            Ok(())
        }
    }

    /// Exact square root if it exists: perfect squares of rationals,
    /// `√(-4) = 2i`, and exact Gaussian roots such as `√(3+4i) = 2+i`.
    pub fn sqrt_exact(&self) -> Option<Number> {
        match self {
            Number::Integer(_) | Number::Rational(_) => {
                let r = self.to_rational()?;
                if r.is_negative() {
                    let root = exact_root(&(-r), 2)?;
                    Some(Number::complex(Number::zero(), Number::from_ratio(root)))
                } else {
                    exact_root(&r, 2).map(Number::from_ratio)
                }
            }
            Number::Complex(c) if self.is_exact() => complex::sqrt_exact(c.re(), c.im()),
            _ => None,
        }
    }

    /// Principal square root; `√(-4) = 2i`.
    pub fn sqrt(&self, prec: Precision) -> NumResult<Number> {
        if let Some(v) = self.sqrt_exact() {
            return Ok(v);
        }
        match self {
            Number::Complex(c) => complex::sqrt(c.re(), c.im(), prec),
            x if x.is_negative() => {
                let r = (-x).sqrt(prec)?;
                Ok(Number::complex(Number::zero(), r))
            }
            x => x.real_map(prec, |b| Ok(b.sqrt().expect("non-negative")), f64::sqrt),
        }
    }

    /// `e^self`.
    pub fn exp(&self, prec: Precision) -> NumResult<Number> {
        match self {
            x if x.is_exact_zero() => Ok(Number::one()),
            Number::Complex(c) => complex::exp(c.re(), c.im(), prec),
            x => x.real_map(prec, BigFloat::exp, f64::exp),
        }
    }

    /// Natural logarithm (principal branch: `ln(-1) = iπ`).
    pub fn ln(&self, prec: Precision) -> NumResult<Number> {
        if self.is_zero() {
            return Err(NumberError::Domain("ln(0)"));
        }
        match self {
            x if x.is_exact_one() => Ok(Number::zero()),
            Number::Complex(c) => complex::ln(c.re(), c.im(), prec),
            x if x.is_negative() => {
                let re = (-x).ln(prec)?;
                let pi = match x {
                    Number::F64(_) => Number::F64(std::f64::consts::PI),
                    _ => Number::pi(x.working_prec(prec))?,
                };
                Ok(Number::complex(re, pi))
            }
            x => x.real_map(prec, BigFloat::ln, f64::ln),
        }
    }

    /// Base-10 logarithm (exact for exact powers of ten).
    pub fn log10(&self, prec: Precision) -> NumResult<Number> {
        self.log(&Number::from(10), prec)
    }

    /// Logarithm in base `b`. Exact when `self = b^k` for an integer `k`.
    pub fn log(&self, b: &Number, prec: Precision) -> NumResult<Number> {
        if b.is_zero() || *b == Number::one() {
            return Err(NumberError::Domain("invalid logarithm base"));
        }
        if self.is_zero() {
            return Err(NumberError::Domain("log(0)"));
        }
        if let Some(k) = exact_log(self, b) {
            return Ok(k);
        }
        let prec = self.working_prec(b.working_prec(prec));
        let positive = |x: &Number| x.is_real() && !x.is_negative();
        if positive(self) && positive(b) {
            if matches!(self, Number::F64(_)) || matches!(b, Number::F64(_)) {
                return Number::f64(self.to_f64().ln() / b.to_f64().ln());
            }
            let x = self.to_bigfloat(prec).expect("real");
            let base = b.to_bigfloat(prec).expect("real");
            return x.log(&base).map(Number::Float);
        }
        self.ln(prec)?.checked_div(&b.ln(prec)?)
    }

    /// Sine (radians).
    pub fn sin(&self, prec: Precision) -> NumResult<Number> {
        match self {
            x if x.is_exact_zero() => Ok(Number::zero()),
            Number::Complex(c) => complex::sin(c.re(), c.im(), prec),
            x => x.real_map(prec, BigFloat::sin, f64::sin),
        }
    }

    /// Cosine (radians).
    pub fn cos(&self, prec: Precision) -> NumResult<Number> {
        match self {
            x if x.is_exact_zero() => Ok(Number::one()),
            Number::Complex(c) => complex::cos(c.re(), c.im(), prec),
            x => x.real_map(prec, BigFloat::cos, f64::cos),
        }
    }

    /// Tangent (radians).
    pub fn tan(&self, prec: Precision) -> NumResult<Number> {
        match self {
            x if x.is_exact_zero() => Ok(Number::zero()),
            Number::Complex(_) => self.sin(prec)?.checked_div(&self.cos(prec)?),
            x => x.real_map(prec, BigFloat::tan, f64::tan),
        }
    }

    fn check_unit_interval(&self, what: &'static str) -> NumResult<()> {
        self.require_real()?;
        if self.abs().cmp_real(&Number::one())? == Ordering::Greater {
            return Err(NumberError::Domain(what));
        }
        Ok(())
    }

    /// Arcsine, real domain `[-1, 1]`.
    pub fn asin(&self, prec: Precision) -> NumResult<Number> {
        self.check_unit_interval("asin outside [-1, 1]")?;
        if self.is_exact_zero() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::asin, f64::asin)
    }

    /// Arccosine, real domain `[-1, 1]`.
    pub fn acos(&self, prec: Precision) -> NumResult<Number> {
        self.check_unit_interval("acos outside [-1, 1]")?;
        if self.is_exact_one() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::acos, f64::acos)
    }

    /// Arctangent.
    pub fn atan(&self, prec: Precision) -> NumResult<Number> {
        if self.is_exact_zero() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::atan, f64::atan)
    }

    /// Two-argument arctangent of reals, in `(-π, π]`; `atan2(0, 0) = 0`.
    pub fn atan2(y: &Number, x: &Number, prec: Precision) -> NumResult<Number> {
        y.require_real()?;
        x.require_real()?;
        if y.is_exact_zero() && !x.is_negative() {
            return Ok(Number::zero());
        }
        if matches!(y, Number::F64(_)) || matches!(x, Number::F64(_)) {
            return Number::f64(y.to_f64().atan2(x.to_f64()));
        }
        let prec = y.working_prec(x.working_prec(prec));
        let bits = prec.bits();
        let fy = y.to_bigfloat(prec).expect("real").with_prec(bits);
        let fx = x.to_bigfloat(prec).expect("real").with_prec(bits);
        BigFloat::atan2(&fy, &fx).map(Number::Float)
    }

    /// Hyperbolic sine.
    pub fn sinh(&self, prec: Precision) -> NumResult<Number> {
        if self.is_exact_zero() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::sinh, f64::sinh)
    }

    /// Hyperbolic cosine.
    pub fn cosh(&self, prec: Precision) -> NumResult<Number> {
        if self.is_exact_zero() {
            return Ok(Number::one());
        }
        self.real_map(prec, BigFloat::cosh, f64::cosh)
    }

    /// Hyperbolic tangent.
    pub fn tanh(&self, prec: Precision) -> NumResult<Number> {
        if self.is_exact_zero() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::tanh, f64::tanh)
    }

    /// Inverse hyperbolic sine.
    pub fn asinh(&self, prec: Precision) -> NumResult<Number> {
        if self.is_exact_zero() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::asinh, f64::asinh)
    }

    /// Inverse hyperbolic cosine, real domain `[1, ∞)`.
    pub fn acosh(&self, prec: Precision) -> NumResult<Number> {
        self.require_real()?;
        if self.cmp_real(&Number::one())? == Ordering::Less {
            return Err(NumberError::Domain("acosh below 1"));
        }
        if self.is_exact_one() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::acosh, f64::acosh)
    }

    /// Inverse hyperbolic tangent, real domain `(-1, 1)`.
    pub fn atanh(&self, prec: Precision) -> NumResult<Number> {
        self.require_real()?;
        if self.abs().cmp_real(&Number::one())? != Ordering::Less {
            return Err(NumberError::Domain("atanh outside (-1, 1)"));
        }
        if self.is_exact_zero() {
            return Ok(Number::zero());
        }
        self.real_map(prec, BigFloat::atanh, f64::atanh)
    }

    /// Exact power, if the result is exactly representable.
    ///
    /// * integer exponents: always exact for exact bases (`(2/3)^-2 = 9/4`,
    ///   `0^0 = 1`, `(1+i)^2 = 2i`);
    /// * rational exponents `p/q`: exact when the base is a perfect `q`-th
    ///   power (`8^(1/3) = 2`, `4^(3/2) = 8`, `(-4)^(1/2) = 2i`).
    ///
    /// Returns `Ok(None)` when the value is not exact (e.g. `2^(1/2)`), when an
    /// operand is inexact, or when the exact result would be astronomically
    /// large, so the caller can keep the power symbolic.
    pub fn pow_exact(&self, e: &Number) -> NumResult<Option<Number>> {
        if !self.is_exact() || !e.is_exact() {
            return Ok(None);
        }
        match e {
            Number::Integer(n) => match self.powi(n) {
                Ok(v) => Ok(Some(v)),
                Err(NumberError::Limit) => Ok(None),
                Err(err) => Err(err),
            },
            Number::Rational(r) => {
                let Some(base) = self.to_rational() else {
                    return Ok(None);
                };
                let (p, q) = (r.numer(), r.denom());
                if base.is_zero() {
                    return if p.is_negative() {
                        Err(NumberError::DivisionByZero)
                    } else {
                        Ok(Some(Number::zero()))
                    };
                }
                let Some(q) = q.to_u32() else {
                    return Ok(None);
                };
                let (abs_base, i_power) = if base.is_negative() {
                    if q != 2 {
                        // Principal value e^(iπp/q) is not a Gaussian rational.
                        return Ok(None);
                    }
                    (-base, true)
                } else {
                    (base, false)
                };
                let Some(root) = exact_root(&abs_base, q) else {
                    return Ok(None);
                };
                let value = match Number::from_ratio(root).powi(p) {
                    Ok(v) => v,
                    Err(NumberError::Limit) => return Ok(None),
                    Err(err) => return Err(err),
                };
                if i_power {
                    // (-a)^(p/2) = a^(p/2) · i^p
                    let k = p.mod_floor_4();
                    let unit = [Number::one(), Number::i(), Number::from(-1), -Number::i()];
                    Ok(Some(&value * &unit[k]))
                } else {
                    Ok(Some(value))
                }
            }
            _ => Ok(None),
        }
    }

    /// Power `self^e` (principal branch). Exact when possible, otherwise
    /// numeric at `prec`. `0^0 = 1`; `0^e` for `e < 0` is a division by zero.
    pub fn pow(&self, e: &Number, prec: Precision) -> NumResult<Number> {
        if let Some(v) = self.pow_exact(e)? {
            return Ok(v);
        }
        let prec = self.working_prec(e.working_prec(prec));
        if let Number::Integer(n) = e {
            match self.powi(n) {
                Ok(v) => return Ok(v),
                Err(NumberError::Limit) if self.is_exact() => {
                    return self.to_float(prec).powi(n);
                }
                Err(err) => return Err(err),
            }
        }
        if self.is_zero() {
            let positive = match e {
                Number::Complex(c) => c.re().signum_i32() == Some(1),
                _ => e.signum_i32() == Some(1),
            };
            return match e.signum_i32() {
                Some(0) => Ok(Number::one()),
                _ if positive => Ok(self.clone()),
                Some(_) => Err(NumberError::DivisionByZero),
                None => Err(NumberError::Domain("0 to a complex power")),
            };
        }
        if self.is_real() && e.is_real() {
            if !self.is_negative() {
                if matches!(self, Number::F64(_)) || matches!(e, Number::F64(_)) {
                    let v = self.to_f64().powf(e.to_f64());
                    if v.is_finite() {
                        return Ok(Number::F64(v));
                    }
                }
                // The exponent amplifies input rounding errors by |e|.
                let bits = prec.bits();
                let amp = e.to_f64().abs().log2().ceil();
                let extra = if amp.is_finite() {
                    amp.clamp(0.0, 128.0) as u64
                } else {
                    0
                };
                let wide = bits + extra + 8;
                let to_big = |v: &Number, extra_in: u64| match v {
                    Number::Float(f) => f.clone(),
                    Number::F64(x) => BigFloat::from_f64(*x, 53).expect("finite"),
                    exact => exact.exact_to_bigfloat(wide + extra_in),
                };
                let x = to_big(self, self.near_one_extra(bits)).with_prec(wide);
                let y = to_big(e, 0);
                return Ok(Number::Float(x.pow(&y)?.with_prec(bits)));
            }
            if e.is_integral() {
                // Negative base, integral inexact exponent: real result.
                let n = e.integer_value()?;
                let v = match self.powi(&n) {
                    Err(NumberError::Limit) if self.is_exact() => self.to_float(prec).powi(&n)?,
                    other => other?,
                };
                return Ok(match (self, e) {
                    (Number::F64(_), _) | (_, Number::F64(_)) => v.into_f64_kind(),
                    _ => v.to_float(prec),
                });
            }
        }
        // Principal branch: z^w = exp(w · ln z)
        (e * &self.ln(prec)?).exp(prec)
    }

    /// Real `n`-th root: like `self^(1/n)`, but odd roots of negative reals
    /// are real (`nthroot(-8, 3) = -2`).
    pub fn nth_root(&self, n: &Number, prec: Precision) -> NumResult<Number> {
        if n.is_zero() {
            return Err(NumberError::Domain("0-th root"));
        }
        let inv = n.recip()?;
        if let Number::Integer(k) = n
            && self.is_negative()
            && num_integer::Integer::is_odd(k)
        {
            return Ok(-(-self).pow(&inv, prec)?);
        }
        self.pow(&inv, prec)
    }
}

trait ModFloor4 {
    fn mod_floor_4(&self) -> usize;
}

impl ModFloor4 for BigInt {
    fn mod_floor_4(&self) -> usize {
        num_integer::Integer::mod_floor(self, &BigInt::from(4))
            .to_usize()
            .expect("0..4")
    }
}

/// Exact `q`-th root of a non-negative rational.
fn exact_root(r: &BigRational, q: u32) -> Option<BigRational> {
    let root = |n: &BigInt| -> Option<BigInt> {
        let c = n.nth_root(q);
        (num_traits::pow::pow(c.clone(), q as usize) == *n).then_some(c)
    };
    Some(BigRational::new(root(r.numer())?, root(r.denom())?))
}

/// Integer `k` with `b^k = x` for exact positive rationals, if any.
fn exact_log(x: &Number, b: &Number) -> Option<Number> {
    let (xr, br) = (x.to_rational()?, b.to_rational()?);
    if !xr.is_positive() || !br.is_positive() {
        return None;
    }
    let estimate = x.to_f64().ln() / b.to_f64().ln();
    let k = if estimate.is_finite() {
        estimate.round()
    } else {
        // Fall back to bit lengths for huge values.
        let bits = |r: &BigRational| r.numer().bits() as f64 - r.denom().bits() as f64;
        (bits(&xr) / bits(&br)).round()
    };
    if !k.is_finite() || k.abs() > 1e6 {
        return None;
    }
    let k = BigInt::from(k as i64);
    let candidate = b.powi(&k).ok()?;
    (candidate == *x).then_some(Number::Integer(k))
}
