//! Limits: direct substitution, series expansion (Laurent series at a
//! point or in `1/x` at infinity), limits of the parts with
//! extended-real arithmetic (`e^x → ∞`, `atan x → π/2`), and
//! L'Hôpital's rule for `0/0`, `∞/∞` and `0·∞`.
//!
//! Infinity is the symbol `∞` (a variable named `∞`): `limit(1/x^2, x,
//! 0) = ∞`, `limit(-x, x, ∞) = -∞`.

use std::cmp::Ordering;

use super::algebra::numer_denom;
use super::approx::{eval_f64, sign};
use super::diff::diff;
use super::expr::{Const, Func, Node, Sym};
use super::series::{series_at, series_at_infinity};
use super::simplify::simplify;
use super::steps::record;

/// Where the limit is taken.
#[derive(Clone, Debug, PartialEq)]
pub enum Point {
    Finite(Sym),
    PosInf,
    NegInf,
}

/// Side of a finite point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Both,
    Plus,
    Minus,
}

/// An extended real value.
#[derive(Clone, Debug, PartialEq)]
pub enum Lim {
    Fin(Sym),
    PosInf,
    NegInf,
}

impl Lim {
    /// `∞` is the symbol named `∞`.
    pub fn to_sym(&self) -> Sym {
        match self {
            Lim::Fin(s) => s.clone(),
            Lim::PosInf => infinity(),
            Lim::NegInf => -infinity(),
        }
    }
}

/// The infinity symbol.
pub fn infinity() -> Sym {
    Sym::var("∞")
}

/// Recognizes `∞` / `-∞` symbols.
pub fn as_infinity(s: &Sym) -> Option<i64> {
    if s.as_var() == Some("∞") {
        return Some(1);
    }
    if let Node::Mul(fs) = s.node()
        && fs.len() == 2
        && fs[1].as_var() == Some("∞")
        && fs[0]
            .as_num()
            .is_some_and(|n| n.is_real() && n.is_negative())
    {
        return Some(-1);
    }
    None
}

/// Why a limit could not be found.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitError(pub String);

type R<T> = Result<T, LimitError>;

fn fail<T>(m: impl Into<String>) -> R<T> {
    Err(LimitError(m.into()))
}

/// `lim_{x → point} e` (from `dir` for a finite point).
pub fn limit(e: &Sym, x: &str, point: &Point, dir: Dir) -> R<Lim> {
    let r = lim_rec(e, x, point, dir, 0)?;
    let r = match r {
        Lim::Fin(s) => Lim::Fin(simplify(&s)),
        other => other,
    };
    record("limit", || (e.clone(), r.to_sym()));
    Ok(r)
}

fn has_discontinuous(e: &Sym) -> bool {
    matches!(e.node(), Node::Fn(Func::Floor | Func::Ceil | Func::Sign, _))
        || e.children().iter().any(has_discontinuous)
}

fn lim_rec(e: &Sym, x: &str, point: &Point, dir: Dir, depth: u32) -> R<Lim> {
    if depth > 8 {
        return fail("limit too complicated");
    }
    if !e.has_var(x) {
        return Ok(Lim::Fin(e.clone()));
    }
    // 1. Direct substitution.
    if let Point::Finite(a) = point
        && !has_discontinuous(e)
    {
        let s = e.subs(x, a);
        if !s.has_undefined() && s.is_constant() && super::approx::approx_f64(&s).is_some() {
            record("direct substitution", || (e.clone(), s.clone()));
            return Ok(Lim::Fin(s));
        }
    }
    // 2. Series.
    if let Some(l) = by_series(e, x, point, dir)? {
        return Ok(l);
    }
    // 3. Parts, then L'Hôpital.
    if let Some(l) = parts(e, x, point, dir, depth)? {
        return Ok(l);
    }
    if let Some(l) = lhopital(e, x, point, dir, depth)? {
        return Ok(l);
    }
    fail("could not determine the limit")
}

fn by_series(e: &Sym, x: &str, point: &Point, dir: Dir) -> R<Option<Lim>> {
    let (s, side) = match point {
        Point::Finite(a) => (series_at(e, x, a, 2), dir),
        Point::PosInf => (series_at_infinity(e, x, 1, 2), Dir::Plus),
        Point::NegInf => (series_at_infinity(e, x, -1, 2), Dir::Plus),
    };
    let Some(s) = s else { return Ok(None) };
    let Some((c, k)) = s.lead() else {
        return Ok(Some(Lim::Fin(Sym::zero())));
    };
    record("series expansion", || {
        (e.clone(), c.clone() * Sym::var("t").powi(k))
    });
    if k > 0 {
        return Ok(Some(Lim::Fin(Sym::zero())));
    }
    if k == 0 {
        return Ok(Some(Lim::Fin(c)));
    }
    let Some(sc) = sign(&c) else {
        return Ok(None);
    };
    let pos = sc == Ordering::Greater;
    let odd = k % 2 != 0;
    let lim = |p: bool| if p { Lim::PosInf } else { Lim::NegInf };
    Ok(Some(match side {
        Dir::Plus => lim(pos),
        Dir::Minus => lim(pos != odd),
        Dir::Both if odd => {
            return fail("the limit does not exist (the one-sided limits differ)");
        }
        Dir::Both => lim(pos),
    }))
}

/// Sign of `e` just beside the point (numerically).
fn side_sign(e: &Sym, x: &str, point: &Point, dir: Dir) -> Option<Ordering> {
    let t = match point {
        Point::Finite(a) => {
            let a = super::approx::approx_f64(a)?;
            let h = 1e-7 * a.abs().max(1.0);
            match dir {
                Dir::Minus => a - h,
                _ => a + h,
            }
        }
        Point::PosInf => 1e12,
        Point::NegInf => -1e12,
    };
    let v = eval_f64(e, x, t);
    if dir == Dir::Both
        && let Point::Finite(a) = point
    {
        let a = super::approx::approx_f64(a)?;
        let w = eval_f64(e, x, a - 1e-7 * a.abs().max(1.0));
        if w.signum() != v.signum() {
            return None;
        }
    }
    v.partial_cmp(&0.0)
}

fn add_lim(a: Lim, b: Lim) -> Option<Lim> {
    Some(match (a, b) {
        (Lim::Fin(p), Lim::Fin(q)) => Lim::Fin(p + q),
        (Lim::PosInf, Lim::NegInf) | (Lim::NegInf, Lim::PosInf) => return None,
        (Lim::PosInf, _) | (_, Lim::PosInf) => Lim::PosInf,
        (Lim::NegInf, _) | (_, Lim::NegInf) => Lim::NegInf,
    })
}

fn mul_lim(a: Lim, b: Lim) -> Option<Lim> {
    let inf_sign = |l: &Lim| match l {
        Lim::PosInf => Some(Ordering::Greater),
        Lim::NegInf => Some(Ordering::Less),
        Lim::Fin(s) => sign(s),
    };
    match (&a, &b) {
        (Lim::Fin(p), Lim::Fin(q)) => Some(Lim::Fin(p * q)),
        _ => {
            let (sa, sb) = (inf_sign(&a)?, inf_sign(&b)?);
            if sa == Ordering::Equal || sb == Ordering::Equal {
                return None;
            }
            Some(if (sa == Ordering::Greater) == (sb == Ordering::Greater) {
                Lim::PosInf
            } else {
                Lim::NegInf
            })
        }
    }
}

/// Limit from the limits of the parts.
fn parts(e: &Sym, x: &str, point: &Point, dir: Dir, depth: u32) -> R<Option<Lim>> {
    let sub = |s: &Sym| -> R<Option<Lim>> {
        match lim_rec(s, x, point, dir, depth + 1) {
            Ok(l) => Ok(Some(l)),
            Err(_) => Ok(None),
        }
    };
    Ok(match e.node() {
        Node::Var(_) => Some(match point {
            Point::Finite(a) => Lim::Fin(a.clone()),
            Point::PosInf => Lim::PosInf,
            Point::NegInf => Lim::NegInf,
        }),
        Node::Add(ts) => {
            let mut acc = Lim::Fin(Sym::zero());
            for t in ts {
                let Some(l) = sub(t)? else { return Ok(None) };
                let Some(n) = add_lim(acc, l) else {
                    return Ok(None);
                };
                acc = n;
            }
            Some(acc)
        }
        Node::Mul(fs) => {
            let mut acc = Lim::Fin(Sym::one());
            for f in fs {
                let Some(l) = sub(f)? else { return Ok(None) };
                let Some(n) = mul_lim(acc, l) else {
                    return Ok(None);
                };
                acc = n;
            }
            Some(acc)
        }
        Node::Pow(b, k) if !k.has_var(x) => {
            let Some(lb) = sub(b)? else { return Ok(None) };
            let ks = sign(k);
            match lb {
                Lim::Fin(v) => {
                    if v.is_exact_zero() && ks == Some(Ordering::Less) {
                        match side_sign(b, x, point, dir) {
                            Some(Ordering::Greater) => Some(Lim::PosInf),
                            Some(Ordering::Less) => {
                                if k.as_integer()
                                    .is_some_and(|i| i.bits() > 0 && (i % 2u32) == 0u32.into())
                                {
                                    Some(Lim::PosInf)
                                } else {
                                    Some(Lim::NegInf)
                                }
                            }
                            _ => None,
                        }
                    } else {
                        let r = Sym::pow(v, k.clone());
                        (!r.has_undefined()).then_some(Lim::Fin(r))
                    }
                }
                Lim::PosInf => match ks {
                    Some(Ordering::Greater) => Some(Lim::PosInf),
                    Some(Ordering::Less) => Some(Lim::Fin(Sym::zero())),
                    _ => None,
                },
                Lim::NegInf => match (ks, k.as_integer()) {
                    (Some(Ordering::Less), _) => Some(Lim::Fin(Sym::zero())),
                    (Some(Ordering::Greater), Some(i)) => Some(if i % 2u32 == 0u32.into() {
                        Lim::PosInf
                    } else {
                        Lim::NegInf
                    }),
                    _ => None,
                },
            }
        }
        Node::Pow(b, u) if !b.has_var(x) => {
            let Some(lu) = sub(u)? else { return Ok(None) };
            let big = if matches!(b.node(), Node::Const(Const::E)) {
                Some(Ordering::Greater)
            } else {
                sign(&(b - Sym::one()))
            };
            match (lu, big) {
                (Lim::Fin(v), _) => Some(Lim::Fin(Sym::pow(b.clone(), v))),
                (Lim::PosInf, Some(Ordering::Greater)) => Some(Lim::PosInf),
                (Lim::PosInf, Some(Ordering::Less)) => Some(Lim::Fin(Sym::zero())),
                (Lim::NegInf, Some(Ordering::Greater)) => Some(Lim::Fin(Sym::zero())),
                (Lim::NegInf, Some(Ordering::Less)) => Some(Lim::PosInf),
                _ => None,
            }
        }
        Node::Pow(b, u) => {
            // b^u = e^(u ln b)
            let ex = Sym::mul(vec![u.clone(), b.ln()]);
            match lim_rec(&ex, x, point, dir, depth + 1) {
                Ok(Lim::Fin(v)) => Some(Lim::Fin(v.exp())),
                Ok(Lim::PosInf) => Some(Lim::PosInf),
                Ok(Lim::NegInf) => Some(Lim::Fin(Sym::zero())),
                Err(_) => None,
            }
        }
        Node::Fn(f, u) => {
            if matches!(f, Func::Floor | Func::Ceil | Func::Sign) {
                return Ok(None);
            }
            let Some(lu) = sub(u)? else { return Ok(None) };
            match lu {
                Lim::Fin(v) => {
                    if *f == Func::Ln && v.is_exact_zero() {
                        Some(Lim::NegInf)
                    } else {
                        let r = Sym::func(*f, v);
                        (!r.has_undefined()).then_some(Lim::Fin(r))
                    }
                }
                Lim::PosInf => match f {
                    Func::Ln | Func::Sinh | Func::Cosh | Func::Asinh | Func::Acosh | Func::Abs => {
                        Some(Lim::PosInf)
                    }
                    Func::Atan => Some(Lim::Fin(Sym::pi() / Sym::int(2))),
                    Func::Tanh => Some(Lim::Fin(Sym::one())),
                    _ => None,
                },
                Lim::NegInf => match f {
                    Func::Sinh | Func::Asinh => Some(Lim::NegInf),
                    Func::Cosh | Func::Abs => Some(Lim::PosInf),
                    Func::Atan => Some(Lim::Fin(-Sym::pi() / Sym::int(2))),
                    Func::Tanh => Some(Lim::Fin(Sym::minus_one())),
                    _ => None,
                },
            }
        }
        _ => None,
    })
}

fn is_zero_or_inf(l: &Lim) -> (bool, bool) {
    match l {
        Lim::Fin(s) => (sign(s) == Some(Ordering::Equal) || s.is_exact_zero(), false),
        _ => (false, true),
    }
}

/// L'Hôpital's rule on `n/d` (also `f·g = f/(1/g)` for `0·∞`).
fn lhopital(e: &Sym, x: &str, point: &Point, dir: Dir, depth: u32) -> R<Option<Lim>> {
    let mut forms = Vec::new();
    let (n, d) = numer_denom(e);
    if !d.is_one() {
        forms.push((n, d));
    }
    if let Node::Mul(fs) = e.node() {
        for (i, f) in fs.iter().enumerate() {
            if !f.has_var(x) {
                continue;
            }
            let mut rest = fs.clone();
            rest.remove(i);
            forms.push((Sym::mul(rest), f.recip()));
        }
    }
    for (n, d) in forms {
        let (Ok(ln), Ok(ld)) = (
            lim_rec(&n, x, point, dir, depth + 1),
            lim_rec(&d, x, point, dir, depth + 1),
        ) else {
            continue;
        };
        let (n0, ninf) = is_zero_or_inf(&ln);
        let (d0, dinf) = is_zero_or_inf(&ld);
        if !((n0 && d0) || (ninf && dinf)) {
            continue;
        }
        let (Ok(dn), Ok(dd)) = (diff(&n, x), diff(&d, x)) else {
            continue;
        };
        let q = super::algebra::cancel(&(&dn / &dd));
        record("L'Hôpital's rule", || (n.clone() / d.clone(), q.clone()));
        if let Ok(l) = lim_rec(&q, x, point, dir, depth + 1) {
            return Ok(Some(l));
        }
    }
    Ok(None)
}
