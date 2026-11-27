//! `simplify`: tries several rewritings and keeps the simplest result.
//!
//! Candidates: the expression with simplified subexpressions, the
//! normalized rational function (`cancel`), its factorization, the
//! expansion, Pythagorean trig/hyperbolic rewrites, logarithm and square
//! root rules and denominator rationalization. "Simplest" is the smallest
//! node count, then the shortest text.

use num_bigint::BigInt;
use num_traits::{One, Signed, ToPrimitive};

use super::algebra::{cancel, expand, factor, numer_denom};
use super::expr::{Func, Node, Sym};
use super::steps::record;

fn complexity(e: &Sym) -> (usize, usize) {
    (e.size(), e.to_string().chars().count())
}

/// Simplifies an expression.
pub fn simplify(e: &Sym) -> Sym {
    let r = simplify_rec(e, 0);
    record("simplify", || (e.clone(), r.clone()));
    r
}

fn simplify_rec(e: &Sym, depth: u32) -> Sym {
    if depth > 6 {
        return e.clone();
    }
    match e.node() {
        Node::Num(_) | Node::Const(_) | Node::Var(_) | Node::Undefined => return e.clone(),
        Node::Rel(items, ops) => {
            return Sym::rel(
                items.iter().map(|i| simplify_rec(i, depth + 1)).collect(),
                ops.clone(),
            );
        }
        _ => {}
    }
    // Children first (function arguments, exponents, bases).
    let inner = e.map(&mut |c| simplify_rec(c, depth + 1));
    let inner = sqrt_rules(&inner);
    let mut best = inner.clone();
    let mut consider = |cand: Sym, rule: &str| {
        if cand.has_undefined() && !best.has_undefined() {
            return;
        }
        if complexity(&cand) < complexity(&best) {
            record(rule, || (best.clone(), cand.clone()));
            best = cand;
        }
    };
    let canc = cancel(&inner);
    consider(canc.clone(), "cancel common factors");
    let has_den = !numer_denom(&inner).1.is_one();
    if has_den || matches!(inner.node(), Node::Add(_)) {
        consider(factor(&canc), "factor");
    }
    consider(expand(&inner), "expand");
    if contains_func(&inner, &[Func::Sin, Func::Cos, Func::Tan]) {
        for c in trig_candidates(&inner) {
            consider(c, "Pythagorean identity");
        }
    }
    if contains_func(&inner, &[Func::Sinh, Func::Cosh, Func::Tanh]) {
        for c in hyperbolic_candidates(&inner) {
            consider(c, "hyperbolic identity");
        }
    }
    if contains_func(&inner, &[Func::Ln]) {
        consider(log_rules(&inner), "logarithm rules");
    }
    if let Some(r) = rationalize(&canc) {
        consider(r, "rationalize the denominator");
    }
    best
}

fn contains_func(e: &Sym, fs: &[Func]) -> bool {
    match e.node() {
        Node::Fn(f, a) => fs.contains(f) || contains_func(a, fs),
        _ => e.children().iter().any(|c| contains_func(c, fs)),
    }
}

/// `√(x^2) = |x|`, `(x^4)^(1/2) = x^2`, `(x^(2k))^(1/(2n))` (real
/// variables).
fn sqrt_rules(e: &Sym) -> Sym {
    let mapped = if e.children().is_empty() {
        e.clone()
    } else {
        e.map(&mut sqrt_rules)
    };
    if let Node::Pow(b, x) = mapped.node()
        && let Node::Pow(bb, be) = b.node()
        && let (Some(k), Some(r)) = (be.as_integer(), x.as_rational())
        && k.is_positive()
        && (&k % BigInt::from(2)).is_zero_check()
        && (r.denom() % BigInt::from(2)).is_zero_check()
    {
        // (bb^k)^r = |bb|^(k r) when k even and r has an even denominator.
        let prod = num_rational::BigRational::from_integer(k) * r;
        return Sym::pow(Sym::func(Func::Abs, bb.clone()), Sym::ratio(prod));
    }
    mapped
}

trait ZeroCheck {
    fn is_zero_check(&self) -> bool;
}

impl ZeroCheck for BigInt {
    fn is_zero_check(&self) -> bool {
        num_traits::Zero::is_zero(self)
    }
}

/// Replaces `f(u)^n` (n ≥ 2) by `(1 ∓ g(u)^2)^(n/2)·f(u)^(n mod 2)`.
fn pythagorean(e: &Sym, from: Func, to: Func, sign: i64) -> Sym {
    let mapped = if e.children().is_empty() {
        e.clone()
    } else {
        e.map(&mut |c| pythagorean(c, from, to, sign))
    };
    if let Node::Pow(b, x) = mapped.node()
        && let Node::Fn(f, u) = b.node()
        && *f == from
        && let Some(n) = x.as_i64()
        && n >= 2
    {
        let g2 = u.apply(to).powi(2);
        let base = Sym::one() + Sym::int(sign) * g2;
        return Sym::mul(vec![base.powi(n / 2), b.powi(n % 2)]);
    }
    mapped
}

/// `tan u → sin u / cos u`.
fn tan_to_sin_cos(e: &Sym) -> Sym {
    let mapped = if e.children().is_empty() {
        e.clone()
    } else {
        e.map(&mut tan_to_sin_cos)
    };
    if let Node::Fn(Func::Tan, u) = mapped.node() {
        return u.apply(Func::Sin) / u.apply(Func::Cos);
    }
    mapped
}

fn trig_candidates(e: &Sym) -> Vec<Sym> {
    let base = tan_to_sin_cos(e);
    let mut out = vec![cancel(&base)];
    for (from, to) in [(Func::Sin, Func::Cos), (Func::Cos, Func::Sin)] {
        let r = pythagorean(&base, from, to, -1);
        let c = cancel(&r);
        out.push(factor(&c));
        out.push(c);
    }
    out
}

fn hyperbolic_candidates(e: &Sym) -> Vec<Sym> {
    let mut out = Vec::new();
    // cosh² = 1 + sinh², sinh² = cosh² − 1
    for (from, to, s) in [(Func::Cosh, Func::Sinh, 1), (Func::Sinh, Func::Cosh, -1)] {
        let r = if s == 1 {
            pythagorean(e, from, to, 1)
        } else {
            // sinh² = −(1 − cosh²)
            neg_pythagorean(e)
        };
        let c = cancel(&r);
        out.push(factor(&c));
        out.push(c);
    }
    out
}

/// `sinh(u)^n → (cosh(u)^2 − 1)^(n/2)·sinh(u)^(n mod 2)`.
fn neg_pythagorean(e: &Sym) -> Sym {
    let mapped = if e.children().is_empty() {
        e.clone()
    } else {
        e.map(&mut neg_pythagorean)
    };
    if let Node::Pow(b, x) = mapped.node()
        && let Node::Fn(Func::Sinh, u) = b.node()
        && let Some(n) = x.as_i64()
        && n >= 2
    {
        let base = u.apply(Func::Cosh).powi(2) - Sym::one();
        return Sym::mul(vec![base.powi(n / 2), b.powi(n % 2)]);
    }
    mapped
}

/// `ln(8) → 3 ln 2`, `ln(a^k) → k ln a` (a > 0 constant), `ln(e^x) = x`,
/// then like terms combine (`ln 8 − ln 2 = 2 ln 2`).
fn log_rules(e: &Sym) -> Sym {
    let mapped = if e.children().is_empty() {
        e.clone()
    } else {
        e.map(&mut log_rules)
    };
    if let Node::Fn(Func::Ln, a) = mapped.node() {
        if let Some(r) = a.as_rational()
            && r.is_positive()
        {
            let mut terms = Vec::new();
            for (n, sign) in [(r.numer().clone(), 1), (r.denom().clone(), -1)] {
                if n.is_one() {
                    continue;
                }
                let Some(fs) = crate::number::factorize(&n).ok() else {
                    return mapped;
                };
                for (p, k) in fs {
                    terms.push(Sym::int(sign * k as i64) * Sym::big_int(p).ln());
                }
            }
            if !terms.is_empty() {
                return Sym::add(terms);
            }
        }
        if let Node::Pow(b, x) = a.node()
            && b.is_constant()
            && super::approx::sign(b) == Some(std::cmp::Ordering::Greater)
            && x.is_num()
        {
            return x * &b.ln();
        }
        if let Node::Mul(fs) = a.node()
            && fs.iter().all(|f| {
                f.is_constant() && super::approx::sign(f) == Some(std::cmp::Ordering::Greater)
            })
        {
            return Sym::add(fs.iter().map(|f| log_rules(&f.ln())).collect());
        }
    }
    mapped
}

/// `n/(a + b√c) → n(a − b√c)/(a² − b²c)` for a constant binomial
/// denominator containing a square root.
fn rationalize(e: &Sym) -> Option<Sym> {
    let (n, d) = numer_denom(e);
    let Node::Add(ts) = d.node() else { return None };
    if ts.len() != 2 || !d.is_constant() {
        return None;
    }
    let has_sqrt = |t: &Sym| {
        t.factors().iter().any(|f| {
            matches!(f.node(), Node::Pow(_, x) if x.as_rational().is_some_and(|r| r.denom().to_i64() == Some(2)))
        })
    };
    if !ts.iter().any(has_sqrt) {
        return None;
    }
    let conj = &ts[0] - &ts[1];
    let new_d = expand(&(&d * &conj));
    if !new_d.is_num() {
        return None;
    }
    Some(expand(&(n * conj)) / new_d)
}
