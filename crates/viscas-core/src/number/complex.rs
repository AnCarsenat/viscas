//! Principal-branch complex functions, built on the real functions of the
//! parts.

use std::cmp::Ordering;

use super::{NumResult, Number, Precision};

/// `e^(a+bi) = e^a (cos b + i sin b)`.
pub(super) fn exp(re: &Number, im: &Number, prec: Precision) -> NumResult<Number> {
    let ea = re.exp(prec)?;
    let (c, s) = (im.cos(prec)?, im.sin(prec)?);
    Ok(Number::complex(&ea * &c, &ea * &s))
}

/// `ln z = ln|z| + i·arg z` with `arg z` in `(-π, π]`.
pub(super) fn ln(re: &Number, im: &Number, prec: Precision) -> NumResult<Number> {
    let m2 = re * re + im * im;
    let half = Number::rational(1, 2).expect("non-zero");
    let ln_abs = &m2.ln(prec)? * &half;
    let arg = Number::atan2(im, re, prec)?;
    Ok(Number::complex(ln_abs, arg))
}

/// Exact square root of a Gaussian rational, if it exists:
/// `√(a+bi) = √((|z|+a)/2) + i·sgn(b)·√((|z|−a)/2)`.
pub(super) fn sqrt_exact(re: &Number, im: &Number) -> Option<Number> {
    let r = (re * re + im * im).sqrt_exact()?;
    if !r.is_real() {
        return None;
    }
    let half = Number::rational(1, 2).expect("non-zero");
    let u = (&(&r + re) * &half).sqrt_exact()?;
    let v = (&(&r - re) * &half).sqrt_exact()?;
    if !u.is_real() || !v.is_real() {
        return None;
    }
    let v = if im.is_negative() { -v } else { v };
    Some(Number::complex(u, v))
}

/// Principal square root (real part `>= 0`).
pub(super) fn sqrt(re: &Number, im: &Number, prec: Precision) -> NumResult<Number> {
    if re.is_exact()
        && im.is_exact()
        && let Some(v) = sqrt_exact(re, im)
    {
        return Ok(v);
    }
    let half = Number::rational(1, 2).expect("non-zero");
    let r = Number::complex(re.clone(), im.clone()).abs();
    let two = Number::from(2);
    if re.cmp_real(&Number::zero())? != Ordering::Less {
        let u = (&(&r + re) * &half).sqrt(prec)?;
        let v = im.checked_div(&(&two * &u))?;
        Ok(Number::complex(u, v))
    } else {
        let v = (&(&r - re) * &half).sqrt(prec)?;
        let v = if im.is_negative() { -v } else { v };
        let u = im.checked_div(&(&two * &v))?;
        Ok(Number::complex(u, v))
    }
}

/// `sin(a+bi) = sin a cosh b + i cos a sinh b`.
pub(super) fn sin(re: &Number, im: &Number, prec: Precision) -> NumResult<Number> {
    let (sa, ca) = (re.sin(prec)?, re.cos(prec)?);
    let (chb, shb) = (im.cosh(prec)?, im.sinh(prec)?);
    Ok(Number::complex(&sa * &chb, &ca * &shb))
}

/// `cos(a+bi) = cos a cosh b − i sin a sinh b`.
pub(super) fn cos(re: &Number, im: &Number, prec: Precision) -> NumResult<Number> {
    let (sa, ca) = (re.sin(prec)?, re.cos(prec)?);
    let (chb, shb) = (im.cosh(prec)?, im.sinh(prec)?);
    Ok(Number::complex(&ca * &chb, -(&sa * &shb)))
}
