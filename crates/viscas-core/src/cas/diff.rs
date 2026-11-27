//! Symbolic differentiation.

use super::expr::{Const, Func, Node, Sym};
use super::steps::{enabled, record};

/// Derivative failure (a function without a known derivative).
#[derive(Clone, Debug, PartialEq)]
pub struct DiffError(pub String);

/// `d/dx e`.
pub fn diff(e: &Sym, x: &str) -> Result<Sym, DiffError> {
    if !e.has_var(x) {
        return Ok(Sym::zero());
    }
    let (rule, d) = match e.node() {
        Node::Var(_) => ("d/dx x = 1", Sym::one()),
        Node::Add(ts) => {
            let mut v = Vec::with_capacity(ts.len());
            for t in ts {
                v.push(diff(t, x)?);
            }
            ("sum rule", Sym::add(v))
        }
        Node::Mul(fs) => {
            let (konst, var): (Vec<Sym>, Vec<Sym>) =
                fs.iter().cloned().partition(|f| !f.has_var(x));
            if var.len() == 1 {
                let d = diff(&var[0], x)?;
                ("constant factor", Sym::mul(konst) * d)
            } else {
                let mut terms = Vec::with_capacity(var.len());
                for i in 0..var.len() {
                    let mut prod = konst.clone();
                    for (j, f) in var.iter().enumerate() {
                        prod.push(if i == j { diff(f, x)? } else { f.clone() });
                    }
                    terms.push(Sym::mul(prod));
                }
                ("product rule", Sym::add(terms))
            }
        }
        Node::Pow(b, n) => {
            if !n.has_var(x) {
                let db = diff(b, x)?;
                let rule = if b.as_var() == Some(x) {
                    "power rule"
                } else {
                    "power rule + chain rule"
                };
                (
                    rule,
                    Sym::mul(vec![n.clone(), Sym::pow(b.clone(), n - &Sym::one()), db]),
                )
            } else if !b.has_var(x) {
                let dn = diff(n, x)?;
                let lnb = if matches!(b.node(), Node::Const(Const::E)) {
                    Sym::one()
                } else {
                    b.ln()
                };
                ("exponential rule", Sym::mul(vec![e.clone(), lnb, dn]))
            } else {
                // d(f^g) = f^g (g' ln f + g f'/f)
                let (db, dn) = (diff(b, x)?, diff(n, x)?);
                let inner = dn * b.ln() + n * &db / b;
                ("logarithmic differentiation", e * &inner)
            }
        }
        Node::Fn(f, u) => {
            let du = diff(u, x)?;
            let outer = match f {
                Func::Sin => u.apply(Func::Cos),
                Func::Cos => -u.apply(Func::Sin),
                Func::Tan => u.apply(Func::Tan).powi(2) + Sym::one(),
                Func::Asin => (Sym::one() - u.powi(2)).sqrt().recip(),
                Func::Acos => -(Sym::one() - u.powi(2)).sqrt().recip(),
                Func::Atan => (Sym::one() + u.powi(2)).recip(),
                Func::Sinh => u.apply(Func::Cosh),
                Func::Cosh => u.apply(Func::Sinh),
                Func::Tanh => Sym::one() - u.apply(Func::Tanh).powi(2),
                Func::Asinh => (u.powi(2) + Sym::one()).sqrt().recip(),
                Func::Acosh => (u.powi(2) - Sym::one()).sqrt().recip(),
                Func::Atanh => (Sym::one() - u.powi(2)).recip(),
                Func::Ln => u.recip(),
                Func::Abs => u.apply(Func::Sign),
                Func::Sign | Func::Floor | Func::Ceil => Sym::zero(),
                Func::Gamma => {
                    return Err(DiffError(
                        "the derivative of gamma (digamma) is not supported".into(),
                    ));
                }
            };
            let rule = if u.as_var() == Some(x) {
                f.name()
            } else {
                "chain rule"
            };
            let rule_text: &str = rule;
            (rule_text, outer * du)
        }
        Node::Rel(items, ops) => {
            let mut v = Vec::with_capacity(items.len());
            for it in items {
                v.push(diff(it, x)?);
            }
            ("differentiate both sides", Sym::rel(v, ops.clone()))
        }
        Node::Undefined => return Ok(e.clone()),
        Node::Num(_) | Node::Const(_) => ("constant", Sym::zero()),
    };
    if enabled() {
        let label = if matches!(e.node(), Node::Fn(..)) && rule.len() <= 5 {
            format!("derivative of {rule}")
        } else {
            rule.to_string()
        };
        record(&label, || (e.clone(), d.clone()));
    }
    Ok(d)
}

/// `n`-th derivative.
pub fn diff_n(e: &Sym, x: &str, n: u32) -> Result<Sym, DiffError> {
    let mut d = e.clone();
    for _ in 0..n {
        d = diff(&d, x)?;
    }
    Ok(d)
}
