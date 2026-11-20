//! The gamma function: Lanczos for `f64`, Stirling series with argument shift
//! for arbitrary precision.

use std::sync::Mutex;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Zero};

use super::{BigFloat, NumResult, Number, NumberError, Precision, check_interrupt};

/// Lanczos approximation (g = 7, n = 9), accurate to ~15 digits.
pub(crate) fn gamma_f64(x: f64) -> f64 {
    use std::f64::consts::PI;
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection; sin(πx) via the distance to the nearest integer.
        let n = x.round();
        let s = (PI * (x - n)).sin() * if n % 2.0 == 0.0 { 1.0 } else { -1.0 };
        return PI / (s * gamma_f64(1.0 - x));
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + G + 0.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    // t^(x+0.5) split in two halves to avoid premature overflow.
    let half = t.powf((x + 0.5) / 2.0);
    (2.0 * PI).sqrt() * half * ((-t).exp() * a) * half
}

/// Cached Bernoulli numbers `B_0, B_2, B_4, ...` (even indices only).
static BERNOULLI: Mutex<Vec<BigRational>> = Mutex::new(Vec::new());

/// `B_{2k}` for `k = 0..count`, from the recurrence
/// `Σ_{j=0}^{m} C(m+1, j) B_j = 0` (odd `B_j` vanish except `B_1 = -1/2`).
fn bernoulli_even(count: usize) -> NumResult<Vec<BigRational>> {
    let mut cache = BERNOULLI.lock().unwrap_or_else(|e| e.into_inner());
    if cache.is_empty() {
        cache.push(BigRational::one());
    }
    while cache.len() < count {
        check_interrupt()?;
        let k = cache.len(); // computing B_{2k}, m = 2k
        let m = 2 * k as u64;
        // Σ_{j even < m} C(m+1, j) B_j + C(m+1, 1)·B_1 + C(m+1, m)·B_m = 0
        let mut sum = BigRational::zero();
        let mut binom = BigInt::one(); // C(m+1, j), updated as j increases
        for j in 0..m {
            if j % 2 == 0 {
                sum += &cache[(j / 2) as usize] * &binom;
            } else if j == 1 {
                sum -= BigRational::new(binom.clone(), BigInt::from(2));
            }
            binom = binom * (m + 1 - j) / (j + 1);
        }
        // binom is now C(m+1, m) = m + 1
        cache.push(-sum / binom);
    }
    Ok(cache[..count].to_vec())
}

impl BigFloat {
    /// Gamma function. Poles at non-positive integers are a domain error.
    pub fn gamma(&self) -> NumResult<Self> {
        let prec = self.prec();
        if self.is_integral() && (self.is_zero() || self.is_negative()) {
            return Err(NumberError::Domain("gamma pole at a non-positive integer"));
        }
        if self.magnitude() > 62 {
            // Γ(x) for |x| >= 2^61 is far outside the exponent range:
            // overflow for x > 0, underflow to zero for (non-integer) x < 0.
            return if self.is_negative() {
                Ok(BigFloat::zero(prec))
            } else {
                Err(NumberError::Overflow)
            };
        }
        let half = BigFloat::from_ratio(&1.into(), &2.into(), 4);
        let guard = 32 + (64 - (self.magnitude().unsigned_abs()).leading_zeros() as u64);
        let wp = prec + guard;
        if self.cmp_value(&half) == std::cmp::Ordering::Less {
            // Γ(x) = π / (sin(πx) Γ(1 - x)), with sin(πx) = (-1)^n sin(π(x - n)).
            let n = self.round()?;
            let delta = self.add_prec(&BigFloat::from_bigint(&n, wp).neg(), wp + prec);
            let pi = BigFloat::pi(wp)?;
            let w = wp + (-delta.magnitude()).max(0) as u64;
            let mut s = pi.mul_prec(&delta, w).sin()?;
            if num_integer::Integer::is_odd(&n) {
                s = s.neg();
            }
            let one_minus = BigFloat::one(wp).add_prec(&self.neg(), wp + prec);
            let g = match one_minus.with_prec(wp).gamma() {
                Err(NumberError::Overflow) => return Ok(BigFloat::zero(prec)),
                other => other?,
            };
            let den = s.mul_prec(&g, wp);
            return pi.div_prec(&den, prec).ok_or(NumberError::DivisionByZero);
        }
        // Shift up to z = x + m >= N so the Stirling series converges fast.
        let n_min = (wp / 4 + 10) as i64;
        let fl = self.floor()?;
        let shift = (BigInt::from(n_min) - &fl).max(BigInt::zero());
        let shift: u64 = num_traits::ToPrimitive::to_u64(&shift).expect("bounded shift");
        // ln Γ(z) ~ z ln z: its integer part costs bits of absolute accuracy.
        let x = self.with_prec(wp);
        let z = x.add_prec(&BigFloat::from_i64(shift as i64, 64), wp + 64);
        let zmag = z.magnitude().max(1);
        let extra = (zmag + (64 - zmag.unsigned_abs().leading_zeros() as i64)) as u64 + 8;
        let w = wp + extra;
        let z = z.with_prec(w);
        let ln_z = z.ln()?;
        let mut lg = z
            .sub(&BigFloat::from_ratio(&1.into(), &2.into(), 4))
            .mul_prec(&ln_z, w)
            .sub(&z);
        let two_pi = BigFloat::pi(w)?.mul_pow2(1);
        lg = lg.add(&two_pi.ln()?.mul_pow2(-1));
        let zinv = BigFloat::one(w).div_prec(&z, w).expect("z > 0");
        let zinv2 = zinv.mul_prec(&zinv, w);
        let mut power = zinv;
        let mut count = 16;
        let mut k = 1usize;
        let tol = -((w + 8) as i64);
        loop {
            let b = bernoulli_even(count)?;
            while k < count {
                let b2k = &b[k];
                let coef = BigFloat::from_ratio(b2k.numer(), b2k.denom(), w);
                let d = BigFloat::from_i64((2 * k * (2 * k - 1)) as i64, 64);
                let t = coef.mul_prec(&power, w).div_prec(&d, w).expect("non-zero");
                lg = lg.add(&t);
                if t.is_zero() || t.magnitude() < tol {
                    let mut g = lg.exp()?;
                    // Undo the shift: Γ(x) = Γ(x + m) / (x (x+1) ... (x+m-1)).
                    if shift > 0 {
                        let mut prod = BigFloat::one(w);
                        for i in 0..shift {
                            prod =
                                prod.mul_prec(&x.add_prec(&BigFloat::from_i64(i as i64, 64), w), w);
                        }
                        g = g.div_prec(&prod, w).expect("positive");
                    }
                    return Ok(g.with_prec(prec));
                }
                power = power.mul_prec(&zinv2, w);
                k += 1;
            }
            if count > 4096 {
                return Err(NumberError::Limit);
            }
            count *= 2;
        }
    }
}

impl Number {
    /// Gamma function `Γ(x)` of a real number. Exact for positive integers
    /// (`Γ(n) = (n-1)!`); poles at non-positive integers are a domain error.
    pub fn gamma(&self, prec: Precision) -> NumResult<Number> {
        let pole = || Err(NumberError::Domain("gamma pole at a non-positive integer"));
        match self {
            Number::Complex(_) => Err(NumberError::NotReal),
            Number::Integer(n) => {
                if n <= &BigInt::zero() {
                    return pole();
                }
                super::theory::factorial(&(n - 1u32)).map(Number::Integer)
            }
            Number::F64(x) => {
                if x.fract() == 0.0 && *x <= 0.0 {
                    return pole();
                }
                let v = gamma_f64(*x);
                if v.is_finite() && v != 0.0 {
                    return Ok(Number::F64(v));
                }
                let b = BigFloat::from_f64(*x, 53).expect("finite");
                Ok(Number::Float(b.gamma()?).into_f64_kind())
            }
            Number::Float(f) => f.gamma().map(Number::Float),
            Number::Rational(r) => {
                let bits = prec.bits();
                BigFloat::from_ratio(r.numer(), r.denom(), bits + 16)
                    .gamma()
                    .map(|g| Number::Float(g.with_prec(bits)))
            }
        }
    }
}
