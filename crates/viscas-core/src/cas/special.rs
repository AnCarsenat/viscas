//! Special values of functions: exact trigonometric values at rational
//! multiples of π, inverse trig tables, `ln(e) = 1`, `|−3| = 3`, …

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_integer::Integer as _;
use num_rational::BigRational;
use num_traits::{One, ToPrimitive, Zero};

use super::approx;
use super::expr::{Const, Func, Node, Sym};

fn r(p: i64, q: i64) -> BigRational {
    BigRational::new(BigInt::from(p), BigInt::from(q))
}

fn s(n: i64) -> Sym {
    Sym::int(n).sqrt()
}

/// `a` as `q·π` with rational `q` (`0` counts as `0·π`).
pub(crate) fn pi_multiple(a: &Sym) -> Option<BigRational> {
    match a.node() {
        Node::Num(_) if a.is_exact_zero() => Some(BigRational::zero()),
        Node::Const(Const::Pi) => Some(BigRational::one()),
        Node::Mul(fs) if fs.len() == 2 && matches!(fs[1].node(), Node::Const(Const::Pi)) => {
            fs[0].as_rational()
        }
        _ => None,
    }
}

/// `sin(tπ)` for `t ∈ [0, 1/2]` in the table.
fn sin_table(t: &BigRational) -> Option<Sym> {
    let table: [(BigRational, fn() -> Sym); 11] = [
        (r(0, 1), Sym::zero),
        (r(1, 12), || (s(6) - s(2)) / Sym::int(4)),
        (r(1, 10), || (s(5) - Sym::one()) / Sym::int(4)),
        (r(1, 6), Sym::half),
        (r(1, 5), || {
            (Sym::int(10) - Sym::int(2) * s(5)).sqrt() / Sym::int(4)
        }),
        (r(1, 4), || s(2) / Sym::int(2)),
        (r(3, 10), || (s(5) + Sym::one()) / Sym::int(4)),
        (r(1, 3), || s(3) / Sym::int(2)),
        (r(2, 5), || {
            (Sym::int(10) + Sym::int(2) * s(5)).sqrt() / Sym::int(4)
        }),
        (r(5, 12), || (s(6) + s(2)) / Sym::int(4)),
        (r(1, 2), Sym::one),
    ];
    table.iter().find(|(k, _)| k == t).map(|(_, f)| f())
}

/// `tan(tπ)` for `t ∈ [0, 1/2)` in the table.
fn tan_table(t: &BigRational) -> Option<Sym> {
    let table: [(BigRational, fn() -> Sym); 6] = [
        (r(0, 1), Sym::zero),
        (r(1, 12), || Sym::int(2) - s(3)),
        (r(1, 6), || s(3) / Sym::int(3)),
        (r(1, 4), Sym::one),
        (r(1, 3), || s(3)),
        (r(5, 12), || Sym::int(2) + s(3)),
    ];
    table.iter().find(|(k, _)| k == t).map(|(_, f)| f())
}

fn mod_rat(t: &BigRational, m: i64) -> BigRational {
    let m = BigRational::from_integer(BigInt::from(m));
    let q = (t / &m).floor();
    t - q * m
}

/// Exact `sin(tπ)` if `t` reduces to a table angle.
pub(crate) fn sin_rat(t: &BigRational) -> Option<Sym> {
    let mut t = mod_rat(t, 2);
    let mut neg = false;
    if t >= BigRational::one() {
        t -= BigRational::one();
        neg = true;
    }
    if t > r(1, 2) {
        t = BigRational::one() - t;
    }
    let v = sin_table(&t)?;
    Some(if neg { -v } else { v })
}

pub(crate) fn cos_rat(t: &BigRational) -> Option<Sym> {
    sin_rat(&(t + r(1, 2)))
}

/// Exact `tan(tπ)`; `Some(Undefined)` at odd multiples of π/2.
pub(crate) fn tan_rat(t: &BigRational) -> Option<Sym> {
    let t = mod_rat(t, 1);
    if t == r(1, 2) {
        return Some(Sym::undefined());
    }
    if t > r(1, 2) {
        return tan_table(&(BigRational::one() - t)).map(|v| -v);
    }
    tan_table(&t)
}

/// Splits `a = rest + qπ`.
fn split_pi_term(a: &Sym) -> Option<(Sym, BigRational)> {
    let Node::Add(ts) = a.node() else {
        return None;
    };
    for (k, t) in ts.iter().enumerate() {
        if let Some(q) = pi_multiple(t)
            && !q.is_zero()
        {
            let mut rest = ts.clone();
            rest.remove(k);
            return Some((Sym::add(rest), q));
        }
    }
    None
}

/// The angle table: all `t ∈ [0, 1/2]` with a known exact sine.
fn table_angles() -> Vec<BigRational> {
    [
        (0, 1),
        (1, 12),
        (1, 10),
        (1, 6),
        (1, 5),
        (1, 4),
        (3, 10),
        (1, 3),
        (2, 5),
        (5, 12),
        (1, 2),
    ]
    .iter()
    .map(|&(p, q)| r(p, q))
    .collect()
}

fn asin_value(v: &Sym) -> Option<Sym> {
    for t in table_angles() {
        let sv = sin_table(&t)?;
        if &sv == v {
            return Some(Sym::ratio(t) * Sym::pi());
        }
        if !sv.is_exact_zero() && -&sv == *v {
            return Some(Sym::ratio(-t) * Sym::pi());
        }
    }
    None
}

fn atan_value(v: &Sym) -> Option<Sym> {
    for t in table_angles() {
        if t == r(1, 2) {
            continue;
        }
        let Some(tv) = tan_table(&t) else { continue };
        if &tv == v {
            return Some(Sym::ratio(t) * Sym::pi());
        }
        if !tv.is_exact_zero() && -&tv == *v {
            return Some(Sym::ratio(-t) * Sym::pi());
        }
    }
    None
}

fn one_minus_sq(x: &Sym) -> Sym {
    (Sym::one() - x.powi(2)).sqrt()
}

/// Special value of `f(a)`, or `None` to keep `f(a)` symbolic.
pub(crate) fn func_special(f: Func, a: &Sym) -> Option<Sym> {
    match f {
        Func::Sin | Func::Cos | Func::Tan => trig_special(f, a),
        Func::Asin => {
            if let Node::Fn(Func::Cos, _) = a.node() {
                return None;
            }
            asin_value(a)
        }
        Func::Acos => asin_value(a).map(|v| Sym::half() * Sym::pi() - v),
        Func::Atan => atan_value(a),
        Func::Sinh | Func::Tanh | Func::Asinh | Func::Atanh => a.is_exact_zero().then(Sym::zero),
        Func::Cosh => a.is_exact_zero().then(Sym::one),
        Func::Acosh => a.is_one().then(Sym::zero),
        Func::Ln => ln_special(a),
        Func::Abs => abs_special(a),
        Func::Sign => sign_special(a),
        Func::Floor | Func::Ceil => round_special(f, a),
        Func::Gamma => gamma_special(a),
    }
}

fn trig_special(f: Func, a: &Sym) -> Option<Sym> {
    if let Some(t) = pi_multiple(a) {
        return match f {
            Func::Sin => sin_rat(&t),
            Func::Cos => cos_rat(&t),
            _ => tan_rat(&t),
        };
    }
    if let Some((rest, q)) = split_pi_term(a) {
        // Shifts by multiples of π/2.
        let period = if f == Func::Tan { 1 } else { 2 };
        let q = mod_rat(&q, period);
        let quarter = (&q * BigRational::from_integer(BigInt::from(2))).to_integer();
        if (&q * BigRational::from_integer(BigInt::from(2))).is_integer() {
            let k = quarter.mod_floor(&BigInt::from(4)).to_u32().unwrap_or(0);
            let sin = || rest.apply(Func::Sin);
            let cos = || rest.apply(Func::Cos);
            return Some(match (f, k) {
                (_, 0) => Sym::func(f, rest.clone()),
                (Func::Sin, 1) => cos(),
                (Func::Sin, 2) => -sin(),
                (Func::Sin, 3) => -cos(),
                (Func::Cos, 1) => -sin(),
                (Func::Cos, 2) => -cos(),
                (Func::Cos, 3) => sin(),
                // tan(x + π/2) = −cot x
                _ => -(rest.apply(Func::Tan).recip()),
            });
        }
    }
    match (f, a.node()) {
        (Func::Sin, Node::Fn(Func::Acos, x)) | (Func::Cos, Node::Fn(Func::Asin, x)) => {
            Some(one_minus_sq(x))
        }
        (Func::Sin, Node::Fn(Func::Atan, x)) => Some(x / (Sym::one() + x.powi(2)).sqrt()),
        (Func::Cos, Node::Fn(Func::Atan, x)) => Some((Sym::one() + x.powi(2)).sqrt().recip()),
        (Func::Tan, Node::Fn(Func::Asin, x)) => Some(x / one_minus_sq(x)),
        (Func::Tan, Node::Fn(Func::Acos, x)) => Some(one_minus_sq(x) / x),
        _ => None,
    }
}

fn ln_special(a: &Sym) -> Option<Sym> {
    match a.node() {
        Node::Num(n) => {
            if n.is_exact_one() {
                return Some(Sym::zero());
            }
            if n.is_exact_zero() {
                return Some(Sym::undefined());
            }
            if n.is_real() && n.is_negative() && n.is_exact() {
                // ln(−a) = ln a + iπ (principal branch)
                return Some(Sym::num(-n).ln() + Sym::i() * Sym::pi());
            }
            None
        }
        Node::Const(Const::E) => Some(Sym::one()),
        Node::Pow(b, e) if matches!(b.node(), Node::Const(Const::E)) => Some(e.clone()),
        _ => None,
    }
}

fn abs_special(a: &Sym) -> Option<Sym> {
    match a.node() {
        Node::Num(n) => {
            if n.is_real() {
                return Some(Sym::num(n.abs()));
            }
            if n.is_exact() {
                let (re, im) = (Sym::num(n.re()), Sym::num(n.im()));
                return Some((re.powi(2) + im.powi(2)).sqrt());
            }
            None
        }
        Node::Fn(Func::Abs, _) => Some(a.clone()),
        Node::Pow(_, e) if e.as_integer().is_some_and(|k| k.is_even()) => Some(a.clone()),
        Node::Mul(fs) if fs[0].is_num() && !a.is_constant() => {
            let c = fs[0].as_num()?;
            if c.is_exact_one() {
                return None;
            }
            let rest = Sym::mul(fs[1..].to_vec());
            Some(Sym::func(Func::Abs, Sym::num(c.clone())) * Sym::func(Func::Abs, rest))
        }
        _ if a.is_constant() => match approx::sign(a)? {
            Ordering::Less => Some(-a),
            _ => Some(a.clone()),
        },
        _ => None,
    }
}

fn sign_special(a: &Sym) -> Option<Sym> {
    if !a.is_constant() {
        if let Node::Fn(Func::Abs, _) = a.node() {
            return None;
        }
        return None;
    }
    if let Some(n) = a.as_num()
        && !n.is_real()
    {
        return None;
    }
    Some(Sym::int(match approx::sign(a)? {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }))
}

fn round_special(f: Func, a: &Sym) -> Option<Sym> {
    if let Some(q) = a.as_rational() {
        return Some(Sym::big_int(if f == Func::Floor {
            q.floor().to_integer()
        } else {
            q.ceil().to_integer()
        }));
    }
    if !a.is_constant() {
        return None;
    }
    let v = approx::approx_f64(a)?;
    let rounded = if f == Func::Floor {
        v.floor()
    } else {
        v.ceil()
    };
    // Refuse when too close to an integer to decide.
    if (v - v.round()).abs() < 1e-9 || !rounded.is_finite() || rounded.abs() > 1e15 {
        return None;
    }
    Some(Sym::int(rounded as i64))
}

fn gamma_special(a: &Sym) -> Option<Sym> {
    let q = a.as_rational()?;
    if q.is_integer() {
        let n = q.to_integer().to_i64()?;
        if n <= 0 {
            return Some(Sym::undefined());
        }
        if n > 200 {
            return None;
        }
        let mut acc = BigInt::one();
        for k in 2..n {
            acc *= k;
        }
        return Some(Sym::big_int(acc));
    }
    if q == r(1, 2) {
        return Some(Sym::pi().sqrt());
    }
    None
}
