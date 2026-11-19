//! Exact and decimal formatting.

use std::fmt;

use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

use super::{BigFloat, Number};

/// Significant digits used when displaying an `F64`.
pub const F64_DISPLAY_DIGITS: usize = 15;

/// Decimal exponents from `-SCI_LOW` up to `min(digits, SCI_HIGH) - 1` are
/// written positionally; others in scientific notation.
const SCI_LOW: i64 = 5;
const SCI_HIGH: usize = 21;

/// Rounds `a / b` (both positive) to the nearest integer, halves up.
fn round_div(a: &BigInt, b: &BigInt) -> BigInt {
    (a * 2u32 + b).div_floor(&(b * 2u32))
}

fn pow10(k: u64) -> BigInt {
    num_traits::pow::pow(BigInt::from(10), k as usize)
}

/// Computes the first `digits` significant digits of a positive value, given
/// `scaled(k) = round(|x| · 10^k)` and an estimate of `floor(log10 |x|)`.
/// Returns the digit string and the decimal exponent of its first digit.
fn significant(scaled: impl Fn(i64) -> BigInt, mut d: i64, digits: usize) -> (String, i64) {
    for _ in 0..8 {
        let s = scaled(digits as i64 - 1 - d).to_string();
        match s.len().cmp(&digits) {
            std::cmp::Ordering::Greater => d += 1,
            std::cmp::Ordering::Less => d -= 1,
            std::cmp::Ordering::Equal => return (s, d),
        }
    }
    let s = scaled(digits as i64 - 1 - d).to_string();
    (s, d)
}

/// Lays out a digit string with decimal exponent `d` (of the first digit).
fn layout(neg: bool, digits_str: &str, d: i64, digits: usize) -> String {
    let ds = digits_str.trim_end_matches('0');
    let ds = if ds.is_empty() { "0" } else { ds };
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if d >= -SCI_LOW && d < digits.min(SCI_HIGH) as i64 {
        if d >= 0 {
            let int_len = d as usize + 1;
            if ds.len() <= int_len {
                out.push_str(ds);
                out.extend(std::iter::repeat_n('0', int_len - ds.len()));
            } else {
                out.push_str(&ds[..int_len]);
                out.push('.');
                out.push_str(&ds[int_len..]);
            }
        } else {
            out.push_str("0.");
            out.extend(std::iter::repeat_n('0', (-d - 1) as usize));
            out.push_str(ds);
        }
    } else {
        out.push_str(&ds[..1]);
        if ds.len() > 1 {
            out.push('.');
            out.push_str(&ds[1..]);
        }
        out.push('e');
        out.push_str(&d.to_string());
    }
    out
}

/// Decimal representation of an exact rational with `digits` significant digits.
fn format_rational(r: &BigRational, digits: usize) -> String {
    if r.is_zero() {
        return "0".to_string();
    }
    let (num, den) = (r.numer().abs(), r.denom().clone());
    let est = ((num.bits() as f64 - den.bits() as f64) * std::f64::consts::LOG10_2).floor() as i64;
    let scaled = |k: i64| {
        if k >= 0 {
            round_div(&(&num * pow10(k as u64)), &den)
        } else {
            round_div(&num, &(&den * pow10((-k) as u64)))
        }
    };
    let (s, d) = significant(scaled, est, digits);
    layout(r.is_negative(), &s, d, digits)
}

/// Decimal representation of a float with `digits` significant digits.
pub(crate) fn format_bigfloat(x: &BigFloat, digits: usize) -> String {
    if x.is_zero() {
        return "0".to_string();
    }
    if x.exponent().abs() < 4096 {
        // Small exponent: exact rational path (correctly rounded).
        return format_rational(&x.to_rational(), digits);
    }
    let est = ((x.magnitude() - 1) as f64 * std::f64::consts::LOG10_2).floor() as i64;
    let ax = x.abs();
    let scaled = |k: i64| {
        let wp = (digits as f64 * std::f64::consts::LOG2_10) as u64 + 64 + 64
            - k.unsigned_abs().leading_zeros() as u64;
        let ten = BigFloat::from_i64(10, wp);
        let scale = ten.powi(&BigInt::from(k)).expect("bounded exponent");
        ax.mul_prec(&scale, wp).round().expect("bounded")
    };
    let (s, d) = significant(scaled, est, digits);
    layout(x.is_negative(), &s, d, digits)
}

/// Formats `re + im·i` given a formatter for real parts.
fn format_complex(
    re: &Number,
    im: &Number,
    exact: bool,
    part: impl Fn(&Number) -> String,
) -> String {
    let neg = im.is_negative();
    let abs_im = if neg { -im } else { im.clone() };
    let im_str = match &abs_im {
        x if x.is_exact_one() => "i".to_string(),
        Number::Rational(r) if exact => {
            let num = r.numer();
            let num = if num.is_one() {
                String::new()
            } else {
                num.to_string()
            };
            format!("{num}i/{}", r.denom())
        }
        x => format!("{}i", part(x)),
    };
    if re.is_zero() {
        if neg { format!("-{im_str}") } else { im_str }
    } else {
        let sign = if neg { '-' } else { '+' };
        format!("{} {sign} {im_str}", part(re))
    }
}

impl Number {
    /// Decimal representation with `digits` significant digits (trailing
    /// zeros trimmed; scientific notation for very large or small values).
    /// Exact integers are shown in full when they fit in `digits` digits.
    pub fn to_decimal_string(&self, digits: usize) -> String {
        let digits = digits.max(1);
        match self {
            Number::Integer(n) => {
                let s = n.abs().to_string();
                if s.len() <= digits.min(SCI_HIGH) {
                    n.to_string()
                } else {
                    format_rational(&BigRational::from_integer(n.clone()), digits)
                }
            }
            Number::Rational(r) => format_rational(r, digits),
            Number::Float(f) => format_bigfloat(f, digits),
            Number::F64(x) => {
                let f = BigFloat::from_f64(*x, 53).expect("finite");
                format_bigfloat(&f, digits.min(17))
            }
            Number::Complex(c) => {
                format_complex(c.re(), c.im(), false, |p| p.to_decimal_string(digits))
            }
        }
    }
}

impl fmt::Display for BigFloat {
    /// Shows as many significant digits as the precision holds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = super::Precision::from_bits(self.prec()).get() as usize;
        f.write_str(&format_bigfloat(self, digits))
    }
}

impl fmt::Display for Number {
    /// Exact display for exact values (`-7`, `1/3`, `1/2 + 3i/2`); decimal
    /// display for floats (all their digits; 15 digits for `F64`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Number::Integer(n) => write!(f, "{n}"),
            Number::Rational(r) => write!(f, "{}/{}", r.numer(), r.denom()),
            Number::Float(x) => write!(f, "{x}"),
            Number::F64(_) => f.write_str(&self.to_decimal_string(F64_DISPLAY_DIGITS)),
            Number::Complex(c) => {
                f.write_str(&format_complex(c.re(), c.im(), true, |p| p.to_string()))
            }
        }
    }
}
