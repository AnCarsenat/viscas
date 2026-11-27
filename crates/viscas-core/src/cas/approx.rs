//! Numeric evaluation of symbolic expressions.

use std::cmp::Ordering;

use super::expr::{Const, Func, Node, Sym, work_prec};
use crate::number::{Number, NumberError, Precision};

type R<T> = Result<T, NumberError>;

/// Numeric value of a constant expression at `prec` (exact parts stay
/// exact where the arithmetic allows it). Errors on free variables.
pub fn approx(s: &Sym, prec: Precision) -> R<Number> {
    crate::number::check_interrupt()?;
    Ok(match s.node() {
        Node::Num(n) => n.clone(),
        Node::Const(Const::Pi) => Number::pi(prec)?,
        Node::Const(Const::E) => Number::e(prec)?,
        Node::Var(_) => return Err(NumberError::Domain("expression has free variables")),
        Node::Undefined => return Err(NumberError::Domain("undefined expression")),
        Node::Rel(..) => return Err(NumberError::Domain("a relation is not a number")),
        Node::Add(ts) => {
            let mut acc = Number::zero();
            for t in ts {
                acc = &acc + &approx(t, prec)?;
            }
            acc
        }
        Node::Mul(fs) => {
            let mut acc = Number::one();
            for f in fs {
                acc = &acc * &approx(f, prec)?;
            }
            acc
        }
        Node::Pow(b, e) => {
            let (b, e) = (approx(b, prec)?, approx(e, prec)?);
            real_pow(&b, &e, prec)?
        }
        Node::Fn(f, a) => apply(*f, &approx(a, prec)?, prec)?,
    })
}

/// Real-first power: odd roots of negative reals are real
/// (`(-8)^(1/3) = -2`), like the evaluator in real mode.
fn real_pow(b: &Number, e: &Number, prec: Precision) -> R<Number> {
    if b.is_real()
        && e.is_real()
        && b.is_negative()
        && !e.is_integral()
        && let Some(q) = e.to_rational()
        && q.denom() % 2u32 == num_bigint::BigInt::from(1)
    {
        let mag = (-b).pow(e, prec)?;
        return Ok(if q.numer() % 2u32 == num_bigint::BigInt::from(0) {
            mag
        } else {
            -mag
        });
    }
    b.pow(e, prec)
}

/// Numeric value of `f(x)`.
pub fn apply(f: Func, x: &Number, prec: Precision) -> R<Number> {
    match f {
        Func::Sin => x.sin(prec),
        Func::Cos => x.cos(prec),
        Func::Tan => x.tan(prec),
        Func::Asin => x.asin(prec),
        Func::Acos => x.acos(prec),
        Func::Atan => x.atan(prec),
        Func::Sinh => x.sinh(prec),
        Func::Cosh => x.cosh(prec),
        Func::Tanh => x.tanh(prec),
        Func::Asinh => x.asinh(prec),
        Func::Acosh => x.acosh(prec),
        Func::Atanh => x.atanh(prec),
        Func::Ln => x.ln(prec),
        Func::Abs => Ok(x.abs_prec(prec)),
        Func::Sign => x.sign_prec(prec),
        Func::Floor => x.floor(),
        Func::Ceil => x.ceil(),
        Func::Gamma => x.gamma(prec),
    }
}

/// `f64` value of a constant expression (real part only if real).
pub fn approx_f64(s: &Sym) -> Option<f64> {
    let n = approx(s, Precision::digits(20)).ok()?;
    n.is_real().then(|| n.to_f64()).filter(|v| v.is_finite())
}

/// Sign of a real constant expression, `None` if unknown (complex,
/// undefined or too close to zero to decide numerically).
pub fn sign(s: &Sym) -> Option<Ordering> {
    if let Some(n) = s.as_num() {
        if !n.is_real() {
            return None;
        }
        return Some(n.signum_i32()?.cmp(&0));
    }
    if !s.is_constant() {
        return None;
    }
    let n = approx(s, work_prec()).ok()?;
    if !n.is_real() {
        return None;
    }
    let v = n.to_f64();
    // Exact cancellations (√2·√2 − 2) are already gone; a tiny value here
    // is most likely a hidden zero.
    if v.abs() < 1e-24 {
        return Some(Ordering::Equal);
    }
    v.partial_cmp(&0.0)
}

/// Fast `f64` evaluation with `var = x` (other variables are errors →
/// NaN). Complex intermediate results give NaN.
pub fn eval_f64(s: &Sym, var: &str, x: f64) -> f64 {
    match s.node() {
        Node::Num(n) => {
            if n.is_real() {
                n.to_f64()
            } else {
                f64::NAN
            }
        }
        Node::Const(Const::Pi) => std::f64::consts::PI,
        Node::Const(Const::E) => std::f64::consts::E,
        Node::Var(v) if &**v == var => x,
        Node::Var(_) | Node::Undefined | Node::Rel(..) => f64::NAN,
        Node::Add(ts) => ts.iter().map(|t| eval_f64(t, var, x)).sum(),
        Node::Mul(fs) => fs.iter().map(|t| eval_f64(t, var, x)).product(),
        Node::Pow(b, e) => {
            let b = eval_f64(b, var, x);
            if let Some(k) = e.as_i64()
                && k.unsigned_abs() < 1 << 20
            {
                return b.powi(k as i32);
            }
            if let Some(q) = e.as_rational()
                && b < 0.0
            {
                use num_traits::ToPrimitive;
                let (p, d) = (
                    q.numer().to_i64().unwrap_or(0),
                    q.denom().to_i64().unwrap_or(2),
                );
                if d % 2 == 1 {
                    let m = (-b).powf(p as f64 / d as f64);
                    return if p % 2 == 0 { m } else { -m };
                }
                return f64::NAN;
            }
            b.powf(eval_f64(e, var, x))
        }
        Node::Fn(f, a) => {
            let a = eval_f64(a, var, x);
            match f {
                Func::Sin => a.sin(),
                Func::Cos => a.cos(),
                Func::Tan => a.tan(),
                Func::Asin => a.asin(),
                Func::Acos => a.acos(),
                Func::Atan => a.atan(),
                Func::Sinh => a.sinh(),
                Func::Cosh => a.cosh(),
                Func::Tanh => a.tanh(),
                Func::Asinh => a.asinh(),
                Func::Acosh => a.acosh(),
                Func::Atanh => a.atanh(),
                Func::Ln => a.ln(),
                Func::Abs => a.abs(),
                Func::Sign => {
                    if a == 0.0 {
                        0.0
                    } else {
                        a.signum()
                    }
                }
                Func::Floor => a.floor(),
                Func::Ceil => a.ceil(),
                Func::Gamma => crate::number::gamma_f64(a),
            }
        }
    }
}

/// Replaces constants and exact surds by floats at `prec` (keeps the
/// symbolic structure of non-constant parts): approx-mode display.
pub fn evalf(s: &Sym, prec: Precision) -> Sym {
    if s.as_rational().is_some() {
        return s.clone();
    }
    if s.is_constant() {
        return match approx(s, prec) {
            Ok(n) => Sym::num(n.to_float(prec)),
            Err(_) => s.clone(),
        };
    }
    match s.node() {
        Node::Rel(items, ops) => {
            Sym::rel(items.iter().map(|i| evalf(i, prec)).collect(), ops.clone())
        }
        Node::Pow(b, e) if e.is_num() => Sym::raw_pow(evalf(b, prec), e.clone()),
        Node::Add(v) => Sym::raw_add(v.iter().map(|t| evalf(t, prec)).collect()),
        Node::Mul(v) => Sym::raw_mul(v.iter().map(|t| evalf(t, prec)).collect()),
        Node::Pow(b, e) => Sym::raw_pow(evalf(b, prec), evalf(e, prec)),
        Node::Fn(f, a) => Sym::func(*f, evalf(a, prec)),
        _ => s.clone(),
    }
}
