//! Parsing numeric literals into exact numbers.

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::Zero;

use super::{BigFloat, NumResult, Number, NumberError, Precision};

/// Decimal exponents beyond this magnitude produce a [`Number::Float`] instead
/// of a (huge) exact rational.
const MAX_EXACT_EXPONENT: u64 = 4096;

/// Literals with more digits than this are refused with [`NumberError::Limit`].
pub const MAX_LITERAL_DIGITS: usize = 100_000;

/// Parses a numeric literal.
///
/// Accepted forms (with an optional leading `+`/`-`): integers (`42`),
/// decimals (`0.1`, `.5`, `5.`), scientific notation (`1.2e-3`, `6E23`),
/// and any of those followed by `i` for an imaginary literal (`2i`, `1.5e3i`,
/// or just `i`). `_` may be used as a digit separator (`1_000_000`).
///
/// Results are exact: `0.1` is `1/10` and `1.2e-3` is `3/2500`. Only
/// literals with a decimal exponent larger than 4096 in magnitude become
/// floats (at the default precision).
pub fn parse_literal(text: &str) -> NumResult<Number> {
    let err = || NumberError::Parse(text.to_string());
    let s = text.trim();
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (imag, s) = match s.strip_suffix('i') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let value = if imag && s.is_empty() {
        Number::one()
    } else {
        parse_real(s)?.ok_or_else(err)?
    };
    let value = if neg { -value } else { value };
    Ok(if imag { value.imaginary() } else { value })
}

/// Parses an unsigned real literal (no `i` suffix); `Ok(None)` on bad syntax.
fn parse_real(s: &str) -> NumResult<Option<Number>> {
    if s.len() > MAX_LITERAL_DIGITS + 32 {
        return Err(NumberError::Limit);
    }
    Ok(parse_real_inner(s))
}

fn parse_real_inner(s: &str) -> Option<Number> {
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(pos) => (&s[..pos], Some(&s[pos + 1..])),
        None => (s, None),
    };
    let (int_part, frac_part) = match mantissa.find('.') {
        Some(pos) => (&mantissa[..pos], &mantissa[pos + 1..]),
        None => (mantissa, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let digits_ok = |p: &str| {
        p.bytes().all(|b| b.is_ascii_digit() || b == b'_')
            && !p.starts_with('_')
            && !p.ends_with('_')
    };
    if !digits_ok(int_part) || !digits_ok(frac_part) {
        return None;
    }
    let mut exp10: i64 = match exponent {
        None => 0,
        Some(e) => {
            let digits = e.strip_prefix(['+', '-']).unwrap_or(e);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            e.parse::<i64>().ok()?
        }
    };
    let all_digits: String = int_part
        .chars()
        .chain(frac_part.chars())
        .filter(|c| *c != '_')
        .collect();
    let frac_len = frac_part.chars().filter(|c| *c != '_').count() as i64;
    let mantissa: BigInt = if all_digits.is_empty() {
        BigInt::zero()
    } else {
        all_digits.parse().ok()?
    };
    exp10 = exp10.checked_sub(frac_len)?;
    if mantissa.is_zero() {
        return Some(Number::zero());
    }
    if exp10.unsigned_abs() <= MAX_EXACT_EXPONENT {
        let pow = num_traits::pow::pow(BigInt::from(10), exp10.unsigned_abs() as usize);
        let r = if exp10 >= 0 {
            BigRational::from_integer(mantissa * pow)
        } else {
            BigRational::new(mantissa, pow)
        };
        return Some(Number::from_ratio(r));
    }
    let bits = Precision::DEFAULT.bits().max(mantissa.bits() + 8);
    let ten = BigFloat::from_i64(10, bits + 64);
    let scale = ten.powi(&BigInt::from(exp10)).ok()?;
    let m = BigFloat::from_bigint(&mantissa, bits + 64);
    Some(Number::Float(
        m.mul(&scale).with_prec(Precision::DEFAULT.bits()),
    ))
}
