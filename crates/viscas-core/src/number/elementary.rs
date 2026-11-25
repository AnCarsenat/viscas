//! Elementary functions and constants for [`BigFloat`], computed with
//! fixed-point series at a working precision with guard bits (in the style of
//! mpmath's `libelefun`), then rounded once to the target precision.

use std::sync::Mutex;

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, ToPrimitive, Zero};

use super::float::{BigFloat, MAX_EXP};
use super::{NumResult, NumberError};

/// Guard bits added to the target precision for internal computations.
const GUARD: u64 = 32;

/// Arguments of trigonometric functions are limited to `|x| < 2^MAX_TRIG_MAG`
/// (argument reduction needs that many extra bits of π).
const MAX_TRIG_MAG: i64 = 1 << 16;

/// Cached fixed-point constant: `(bits, value · 2^bits)`.
type Cache = Mutex<Option<(u64, BigInt)>>;

static PI_CACHE: Cache = Mutex::new(None);
static LN2_CACHE: Cache = Mutex::new(None);

fn cached(cache: &Cache, wp: u64, compute: fn(u64) -> NumResult<BigInt>) -> NumResult<BigInt> {
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((bits, value)) = guard.as_ref()
        && *bits >= wp
    {
        return Ok(value >> (bits - wp) as usize);
    }
    let bits = wp.max(256) + 16;
    let value = compute(bits)?;
    let out = &value >> (bits - wp) as usize;
    *guard = Some((bits, value));
    Ok(out)
}

/// `acot(n) · 2^wp` by its Taylor series.
fn acot_fixed(n: u64, wp: u64) -> NumResult<BigInt> {
    let n2 = BigInt::from(n * n);
    let mut term = (BigInt::one() << wp as usize) / n;
    let mut sum = term.clone();
    let mut k: u64 = 1;
    loop {
        super::check_interrupt()?;
        term /= &n2;
        if term.is_zero() {
            break;
        }
        let t = &term / (2 * k + 1);
        if k % 2 == 1 {
            sum -= t;
        } else {
            sum += t;
        }
        k += 1;
    }
    Ok(sum)
}

fn compute_pi(wp: u64) -> NumResult<BigInt> {
    // Machin: π = 16·acot(5) − 4·acot(239)
    let w = wp + 16;
    let pi = (acot_fixed(5, w)? << 4usize) - (acot_fixed(239, w)? << 2usize);
    Ok(pi >> 16usize)
}

fn compute_ln2(wp: u64) -> NumResult<BigInt> {
    // ln 2 = 2·atanh(1/3) = 2·Σ 1/((2k+1)·3^(2k+1))
    let w = wp + 16;
    let mut term = (BigInt::one() << w as usize) / 3u32;
    let mut sum = term.clone();
    let mut k: u64 = 1;
    loop {
        super::check_interrupt()?;
        term /= 9u32;
        if term.is_zero() {
            break;
        }
        sum += &term / (2 * k + 1);
        k += 1;
    }
    Ok((sum << 1usize) >> 16usize)
}

/// `π · 2^wp`.
pub(crate) fn pi_fixed(wp: u64) -> NumResult<BigInt> {
    cached(&PI_CACHE, wp, compute_pi)
}

/// `ln 2 · 2^wp`.
pub(crate) fn ln2_fixed(wp: u64) -> NumResult<BigInt> {
    cached(&LN2_CACHE, wp, compute_ln2)
}

/// Fixed-point product, truncated toward zero (so series terms reach zero).
fn fmul(a: &BigInt, b: &BigInt, wp: u64) -> BigInt {
    let p = a * b;
    if p.sign() == num_bigint::Sign::Minus {
        -((-p) >> wp as usize)
    } else {
        p >> wp as usize
    }
}

fn fdiv(a: &BigInt, b: &BigInt, wp: u64) -> BigInt {
    (a << wp as usize) / b
}

fn fsqrt(a: &BigInt, wp: u64) -> BigInt {
    (a << wp as usize).sqrt()
}

/// `exp(r)` for a small fixed-point `r` (|r| < 1).
fn exp_series(r: &BigInt, wp: u64) -> NumResult<BigInt> {
    let one = BigInt::one() << wp as usize;
    let mut sum = one.clone();
    let mut term = one;
    let mut k: u32 = 1;
    loop {
        super::check_interrupt()?;
        term = fmul(&term, r, wp) / k;
        if term.is_zero() {
            break;
        }
        sum += &term;
        k += 1;
    }
    Ok(sum)
}

/// `(sin r, cos r)` for a fixed-point `|r| <= π/4`.
fn sin_cos_series(r: &BigInt, wp: u64) -> NumResult<(BigInt, BigInt)> {
    let one = BigInt::one() << wp as usize;
    let r2 = fmul(r, r, wp);
    let mut sin = r.clone();
    let mut term = r.clone();
    let mut k: u64 = 1;
    loop {
        super::check_interrupt()?;
        term = -fmul(&term, &r2, wp) / ((2 * k) * (2 * k + 1));
        if term.is_zero() {
            break;
        }
        sin += &term;
        k += 1;
    }
    let mut cos = one.clone();
    let mut term = one;
    let mut k: u64 = 1;
    loop {
        super::check_interrupt()?;
        term = -fmul(&term, &r2, wp) / ((2 * k - 1) * (2 * k));
        if term.is_zero() {
            break;
        }
        cos += &term;
        k += 1;
    }
    Ok((sin, cos))
}

/// `atanh(t)` for a fixed-point `|t| <= 1/2`.
fn atanh_series(t: &BigInt, wp: u64) -> NumResult<BigInt> {
    let t2 = fmul(t, t, wp);
    let mut sum = t.clone();
    let mut term = t.clone();
    let mut k: u64 = 1;
    loop {
        super::check_interrupt()?;
        term = fmul(&term, &t2, wp);
        if term.is_zero() {
            break;
        }
        sum += &term / (2 * k + 1);
        k += 1;
    }
    Ok(sum)
}

/// `atan(t)` for a fixed-point `|t| <= 1`.
fn atan_fixed(t: &BigInt, wp: u64) -> NumResult<BigInt> {
    let one = BigInt::one() << wp as usize;
    // Halve the angle a few times: atan(t) = 2·atan(t / (1 + √(1 + t²))).
    const HALVINGS: usize = 4;
    let mut t = t.clone();
    for _ in 0..HALVINGS {
        let s = fsqrt(&(&one + fmul(&t, &t, wp)), wp);
        t = fdiv(&t, &(&one + s), wp);
    }
    let t2 = fmul(&t, &t, wp);
    let mut sum = t.clone();
    let mut term = t;
    let mut k: u64 = 1;
    loop {
        super::check_interrupt()?;
        term = -fmul(&term, &t2, wp);
        if term.is_zero() {
            break;
        }
        sum += &term / (2 * k + 1);
        k += 1;
    }
    Ok(sum << HALVINGS)
}

/// Extra bits needed for relative accuracy when `f(x) ≈ x` near zero.
fn small_extra(x: &BigFloat) -> u64 {
    if x.is_zero() {
        0
    } else {
        (-x.magnitude()).max(0) as u64
    }
}

impl BigFloat {
    /// π to `prec` bits.
    pub fn pi(prec: u64) -> NumResult<Self> {
        let wp = prec + GUARD;
        Ok(BigFloat::from_fixed(pi_fixed(wp)?, wp, prec))
    }

    /// ln 2 to `prec` bits.
    pub fn ln2(prec: u64) -> NumResult<Self> {
        let wp = prec + GUARD;
        Ok(BigFloat::from_fixed(ln2_fixed(wp)?, wp, prec))
    }

    /// Euler's number e to `prec` bits.
    pub fn e(prec: u64) -> NumResult<Self> {
        BigFloat::one(prec).exp()
    }

    /// `e^self`. Overflows for huge arguments; underflows to zero.
    pub fn exp(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_zero() {
            return Ok(BigFloat::one(prec));
        }
        let mag = self.magnitude();
        if mag > 58 {
            return if self.is_negative() {
                Ok(BigFloat::zero(prec))
            } else {
                Err(NumberError::Overflow)
            };
        }
        let halvings = ((prec as f64).sqrt() as u64 / 2).max(4);
        let wp = prec + GUARD + halvings;
        let w = wp + mag.max(0) as u64 + 8;
        let x = self.to_fixed(w);
        let l = ln2_fixed(w)?;
        // n = round(x / ln 2)
        let n = (&x * 2u32 + &l).div_floor(&(&l * 2u32));
        let r = x - &n * &l;
        let r = r >> halvings as usize;
        let mut s = exp_series(&r, w)?;
        for _ in 0..halvings {
            s = fmul(&s, &s, w);
        }
        let n = n.to_i64().ok_or(NumberError::Overflow)?;
        if n.abs() > MAX_EXP {
            return Err(NumberError::Overflow);
        }
        Ok(BigFloat::from_fixed(s, w, prec).mul_pow2(n))
    }

    /// Natural logarithm; domain `x > 0`.
    pub fn ln(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_zero() {
            return Err(NumberError::Domain("ln(0)"));
        }
        if self.is_negative() {
            return Err(NumberError::Domain("ln of a negative number"));
        }
        let one = BigFloat::one(prec);
        let mag = self.magnitude();
        let extra = if (0..=1).contains(&mag) {
            let d = self.add_prec(&one.neg(), prec + 4);
            if d.is_zero() {
                return Ok(BigFloat::zero(prec));
            }
            small_extra(&d)
        } else {
            0
        };
        let wp = prec + GUARD + extra;
        // x = m·2^e with m in [0.75, 1.5)
        let mut e = mag;
        let mut m = self.mul_pow2(-e);
        if m.cmp_value(&BigFloat::from_ratio(&3.into(), &4.into(), 8)) == std::cmp::Ordering::Less {
            m = m.mul_pow2(1);
            e -= 1;
        }
        let one_fx = BigInt::one() << wp as usize;
        let mf = m.to_fixed(wp);
        let t = fdiv(&(&mf - &one_fx), &(&mf + &one_fx), wp);
        let mut sum = atanh_series(&t, wp)? << 1usize;
        if e != 0 {
            let l = ln2_fixed(wp + 64)?;
            sum += (l * e) >> 64usize;
        }
        Ok(BigFloat::from_fixed(sum, wp, prec))
    }

    /// Base-10 logarithm.
    pub fn log10(&self) -> NumResult<Self> {
        let prec = self.prec();
        let wp = prec + GUARD;
        let ten = BigFloat::from_i64(10, wp);
        let num = self.with_prec(wp).ln()?;
        Ok(num.div_prec(&ten.ln()?, prec).expect("ln 10 != 0"))
    }

    /// Logarithm in base `b` (`b > 0`, `b != 1`).
    pub fn log(&self, b: &BigFloat) -> NumResult<Self> {
        let prec = self.prec().max(b.prec());
        let wp = prec + GUARD;
        let den = b.with_prec(wp).ln()?;
        if den.is_zero() {
            return Err(NumberError::Domain("logarithm base 1"));
        }
        let num = self.with_prec(wp).ln()?;
        Ok(num.div_prec(&den, prec).expect("non-zero"))
    }

    /// `(sin x, cos x)`.
    pub fn sin_cos(&self) -> NumResult<(Self, Self)> {
        let prec = self.prec();
        if self.is_zero() {
            return Ok((BigFloat::zero(prec), BigFloat::one(prec)));
        }
        let mag = self.magnitude();
        if mag > MAX_TRIG_MAG {
            return Err(NumberError::Overflow);
        }
        let need = prec + GUARD / 2;
        let mut wp = prec + GUARD + small_extra(self) + mag.max(0) as u64;
        for _ in 0..8 {
            let x = self.to_fixed(wp);
            let half_pi = pi_fixed(wp)? >> 1usize;
            let n = (&x * 2u32 + &half_pi).div_floor(&(&half_pi * 2u32));
            let r = &x - &n * &half_pi;
            let have = r.bits() as i64 - n.bits() as i64;
            if have < need as i64 {
                wp += (need as i64 - have) as u64 + 16;
                continue;
            }
            let (s, c) = sin_cos_series(&r, wp)?;
            let quadrant = n.mod_floor(&BigInt::from(4)).to_u8().expect("0..4");
            let (s, c) = match quadrant {
                0 => (s, c),
                1 => (c, -s),
                2 => (-s, -c),
                _ => (-c, s),
            };
            return Ok((
                BigFloat::from_fixed(s, wp, prec),
                BigFloat::from_fixed(c, wp, prec),
            ));
        }
        Err(NumberError::Overflow)
    }

    /// Sine.
    pub fn sin(&self) -> NumResult<Self> {
        Ok(self.sin_cos()?.0)
    }

    /// Cosine.
    pub fn cos(&self) -> NumResult<Self> {
        Ok(self.sin_cos()?.1)
    }

    /// Tangent.
    pub fn tan(&self) -> NumResult<Self> {
        let prec = self.prec();
        let (s, c) = self.with_prec(prec + 8).sin_cos()?;
        s.div_prec(&c, prec).ok_or(NumberError::DivisionByZero)
    }

    /// Arctangent, in `(-π/2, π/2)`.
    pub fn atan(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_zero() {
            return Ok(BigFloat::zero(prec));
        }
        let wp = prec + GUARD + small_extra(self);
        let one = BigFloat::one(wp);
        let big = self.abs().cmp_value(&one) == std::cmp::Ordering::Greater;
        let r = if big {
            let inv = one.div_prec(&self.abs(), wp).expect("non-zero");
            let a = atan_fixed(&inv.to_fixed(wp), wp)?;
            let half_pi = pi_fixed(wp)? >> 1usize;
            let v = half_pi - a;
            if self.is_negative() { -v } else { v }
        } else {
            atan_fixed(&self.to_fixed(wp), wp)?
        };
        Ok(BigFloat::from_fixed(r, wp, prec))
    }

    /// Two-argument arctangent `atan2(y, x)` in `(-π, π]`; `atan2(0, 0) = 0`.
    pub fn atan2(y: &Self, x: &Self) -> NumResult<Self> {
        let prec = y.prec().max(x.prec());
        let wp = prec + GUARD;
        if x.is_zero() {
            if y.is_zero() {
                return Ok(BigFloat::zero(prec));
            }
            let hp = BigFloat::pi(prec)?.mul_pow2(-1);
            return Ok(if y.is_negative() { hp.neg() } else { hp });
        }
        let q = y
            .div_prec(x, wp + small_extra(y) + x.magnitude().max(0) as u64)
            .expect("x != 0");
        let a = q.atan()?;
        if !x.is_negative() {
            return Ok(a.with_prec(prec));
        }
        let pi = BigFloat::pi(a.prec())?;
        let r = if y.is_negative() {
            a.sub(&pi)
        } else {
            a.add(&pi)
        };
        Ok(r.with_prec(prec))
    }

    /// Arcsine, domain `[-1, 1]`.
    pub fn asin(&self) -> NumResult<Self> {
        let prec = self.prec();
        let wp = prec + GUARD + small_extra(self);
        let one = BigFloat::one(wp);
        match self.abs().cmp_value(&one) {
            std::cmp::Ordering::Greater => Err(NumberError::Domain("asin outside [-1, 1]")),
            std::cmp::Ordering::Equal => {
                let hp = BigFloat::pi(prec)?.mul_pow2(-1);
                Ok(if self.is_negative() { hp.neg() } else { hp })
            }
            std::cmp::Ordering::Less => {
                let x = self.with_prec(wp);
                let exact = 2 * wp + 64;
                let d = one
                    .add_prec(&x.neg(), exact)
                    .mul_prec(&one.add_prec(&x, exact), wp);
                let s = d.sqrt_prec(wp).expect("positive");
                let q = x.div_prec(&s, wp).expect("non-zero");
                Ok(q.atan()?.with_prec(prec))
            }
        }
    }

    /// Arccosine, domain `[-1, 1]`.
    pub fn acos(&self) -> NumResult<Self> {
        let prec = self.prec();
        let wp = prec + GUARD;
        let one = BigFloat::one(wp);
        if self.abs().cmp_value(&one) == std::cmp::Ordering::Greater {
            return Err(NumberError::Domain("acos outside [-1, 1]"));
        }
        let exact = 2 * wp + 64;
        let den = one.add_prec(self, exact);
        if den.is_zero() {
            return BigFloat::pi(prec);
        }
        let num = one.add_prec(&self.neg(), exact);
        let extra = small_extra(&num);
        let w = wp + extra;
        let q = num.div_prec(&den, w).expect("non-zero");
        let r = q.sqrt_prec(w).expect("non-negative").atan()?;
        Ok(r.mul_pow2(1).with_prec(prec))
    }

    /// Hyperbolic sine. Odd: computed on `|x|`, so `exp` never underflows to
    /// zero (huge `|x|` overflows on both sides).
    pub fn sinh(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_zero() {
            return Ok(BigFloat::zero(prec));
        }
        if self.is_negative() {
            return Ok(self.neg().sinh()?.neg());
        }
        let wp = prec + GUARD + small_extra(self);
        let a = self.with_prec(wp).exp()?;
        let b = BigFloat::one(wp)
            .div_prec(&a, wp)
            .ok_or(NumberError::Overflow)?;
        Ok(a.sub(&b).mul_pow2(-1).with_prec(prec))
    }

    /// Hyperbolic cosine. Even: computed on `|x|` (see [`BigFloat::sinh`]).
    pub fn cosh(&self) -> NumResult<Self> {
        let prec = self.prec();
        let wp = prec + GUARD;
        let a = self.abs().with_prec(wp).exp()?;
        let b = BigFloat::one(wp)
            .div_prec(&a, wp)
            .ok_or(NumberError::Overflow)?;
        Ok(a.add(&b).mul_pow2(-1).with_prec(prec))
    }

    /// Hyperbolic tangent.
    pub fn tanh(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_zero() {
            return Ok(BigFloat::zero(prec));
        }
        if self.magnitude() > 2 + (64 - prec.leading_zeros()) as i64 {
            // |x| > prec · ln2 / 2: tanh(x) rounds to ±1.
            let one = BigFloat::one(prec);
            return Ok(if self.is_negative() { one.neg() } else { one });
        }
        let wp = prec + GUARD + small_extra(self);
        let e2 = self.with_prec(wp).mul_pow2(1).exp()?;
        let one = BigFloat::one(wp);
        let r = e2.sub(&one).div_prec(&e2.add(&one), wp).expect("positive");
        Ok(r.with_prec(prec))
    }

    /// Inverse hyperbolic sine.
    pub fn asinh(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_zero() {
            return Ok(BigFloat::zero(prec));
        }
        let wp = prec + GUARD + small_extra(self);
        let x = self.abs().with_prec(wp);
        let s = x.mul(&x).add(&BigFloat::one(wp)).sqrt().expect("positive");
        let r = x.add(&s).ln()?;
        let r = if self.is_negative() { r.neg() } else { r };
        Ok(r.with_prec(prec))
    }

    /// Inverse hyperbolic cosine, domain `x >= 1`.
    pub fn acosh(&self) -> NumResult<Self> {
        let prec = self.prec();
        let wp = prec + GUARD;
        let one = BigFloat::one(wp);
        if self.cmp_value(&one) == std::cmp::Ordering::Less {
            return Err(NumberError::Domain("acosh below 1"));
        }
        let exact = 2 * wp + 64;
        let x = self.with_prec(wp);
        let d = x.add_prec(&one.neg(), exact);
        let w = wp + small_extra(&d);
        let p = d.mul_prec(&x.add_prec(&one, exact), w);
        let s = p.sqrt_prec(w).expect("non-negative");
        Ok(x.add_prec(&s, w).ln()?.with_prec(prec))
    }

    /// Inverse hyperbolic tangent, domain `(-1, 1)`.
    pub fn atanh(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_zero() {
            return Ok(BigFloat::zero(prec));
        }
        let wp = prec + GUARD + small_extra(self);
        let one = BigFloat::one(wp);
        if self.abs().cmp_value(&one) != std::cmp::Ordering::Less {
            return Err(NumberError::Domain("atanh outside (-1, 1)"));
        }
        let exact = 2 * wp + 64;
        let num = one.add_prec(self, exact);
        let den = one.add_prec(&self.neg(), exact);
        let w = wp + small_extra(&den);
        let q = num.div_prec(&den, w).expect("non-zero");
        Ok(q.ln()?.mul_pow2(-1).with_prec(prec))
    }

    /// `self^y` for `self > 0` (or `self = 0`, `y > 0`), via `exp(y·ln x)`.
    pub fn pow(&self, y: &BigFloat) -> NumResult<Self> {
        let prec = self.prec().max(y.prec());
        if y.is_zero() {
            return Ok(BigFloat::one(prec));
        }
        if self.is_zero() {
            return if y.is_negative() {
                Err(NumberError::DivisionByZero)
            } else {
                Ok(BigFloat::zero(prec))
            };
        }
        if self.is_negative() {
            return Err(NumberError::Domain("negative base with real exponent"));
        }
        if y.is_integral() && y.magnitude() < 62 {
            let n = y.floor()?;
            return self.with_prec(prec).powi(&n);
        }
        // log2|ln x| (up to a small constant), to bound |y·ln x| cheaply.
        let mag_x = self.magnitude();
        let one = BigFloat::one(2);
        let ln_mag = if (0..=1).contains(&mag_x) {
            let d = self.add_prec(&one.neg(), prec + 4);
            if d.is_zero() {
                return Ok(BigFloat::one(prec)); // 1^y
            }
            d.magnitude() + 1
        } else {
            64 - mag_x.unsigned_abs().leading_zeros() as i64 + 1
        };
        let t_mag = y.magnitude().saturating_add(ln_mag);
        if t_mag > 64 {
            // |y·ln x| > 2^62: e^t is far outside the exponent range.
            let grows = (mag_x > 0 && self.cmp_value(&one) == std::cmp::Ordering::Greater)
                != y.is_negative();
            return if grows {
                Err(NumberError::Overflow)
            } else {
                Ok(BigFloat::zero(prec))
            };
        }
        // Bits of the exponent y·ln(x) that are lost to the integer part.
        let extra = t_mag.max(0) as u64;
        let wp = prec + GUARD + extra;
        let l = self.with_prec(wp).ln()?;
        let t = l.mul_prec(y, wp);
        Ok(t.exp()?.with_prec(prec))
    }

    /// `n`-th root of a non-negative number.
    pub fn nth_root(&self, n: u32) -> NumResult<Self> {
        if n == 0 {
            return Err(NumberError::Domain("0-th root"));
        }
        let prec = self.prec();
        let inv = BigFloat::one(prec + GUARD)
            .div_prec(&BigFloat::from_i64(n as i64, 64), prec + GUARD)
            .expect("n != 0");
        self.pow(&inv)
    }
}
