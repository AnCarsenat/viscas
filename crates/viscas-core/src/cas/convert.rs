//! Conversion between the syntax tree ([`Expr`]) and [`Sym`].
//!
//! [`from_expr`] is a purely structural conversion (no environment):
//! identifiers become variables except the constants `π`, `τ`, `e`, `i`;
//! calls of elementary functions map to their symbolic forms. The
//! evaluator does not use it (it builds symbolic values by evaluating);
//! it serves tools and tests. [`to_expr`] goes back through the printer,
//! whose output always re-parses.

use super::expr::{Func, Sym};
use crate::eval::canonical_name;
use crate::number::parse_literal;
use crate::syntax::{AddOp, BinOp, Expr, ExprKind, MulOp, UnaryOp, parse_expr};

/// Structural conversion of an expression.
pub fn from_expr(e: &Expr) -> Result<Sym, String> {
    Ok(match &*e.kind {
        ExprKind::Number { text, imaginary } => {
            let n = parse_literal(text).map_err(|e| e.to_string())?;
            let s = Sym::num(n);
            if *imaginary { s * Sym::i() } else { s }
        }
        ExprKind::Ident(name) => match canonical_name(name) {
            "π" => Sym::pi(),
            "τ" => Sym::int(2) * Sym::pi(),
            "e" => Sym::e(),
            "i" => Sym::i(),
            other => Sym::var(other),
        },
        ExprKind::Unary { op, expr } => {
            let v = from_expr(expr)?;
            match op {
                UnaryOp::Neg => -v,
                UnaryOp::Pos => v,
                UnaryOp::Not => return Err("`not` is not algebraic".into()),
            }
        }
        ExprKind::Binary {
            op: BinOp::Pow,
            lhs,
            rhs,
        } => Sym::pow(from_expr(lhs)?, from_expr(rhs)?),
        ExprKind::Add(terms) => {
            let mut v = Vec::with_capacity(terms.len());
            for t in terms {
                let s = from_expr(&t.expr)?;
                v.push(if t.op == AddOp::Sub { -s } else { s });
            }
            Sym::add(v)
        }
        ExprKind::Mul(fs) => {
            let mut v = Vec::with_capacity(fs.len());
            for f in fs {
                let s = from_expr(&f.expr)?;
                v.push(if f.op == MulOp::Div { s.recip() } else { s });
            }
            Sym::mul(v)
        }
        ExprKind::Call { callee, args } => {
            let ExprKind::Ident(name) = &*callee.kind else {
                return Err("unsupported call".into());
            };
            let mut a = Vec::with_capacity(args.len());
            for x in args {
                a.push(from_expr(x)?);
            }
            apply_named(canonical_name(name), &a)
                .ok_or_else(|| format!("unknown function `{name}`"))?
        }
        ExprKind::Compare { first, rest } => {
            let mut items = vec![from_expr(first)?];
            let mut ops = Vec::new();
            for (op, x) in rest {
                ops.push(*op);
                items.push(from_expr(x)?);
            }
            Sym::rel(items, ops)
        }
        _ => return Err("expression is not algebraic".into()),
    })
}

/// Applies a builtin by name to symbolic arguments (`sqrt`, `exp`,
/// `log`, reciprocal trig functions, …). `None` for unknown names or
/// arities.
pub fn apply_named(name: &str, a: &[Sym]) -> Option<Sym> {
    if let Some(f) = Func::from_name(name) {
        return (a.len() == 1).then(|| Sym::func(f, a[0].clone()));
    }
    let x = a.first()?.clone();
    Some(match (name, a.len()) {
        ("sqrt", 1) => x.sqrt(),
        ("cbrt", 1) => Sym::pow(x, Sym::rat(1, 3)),
        ("root", 2) => Sym::pow(x, a[1].recip()),
        ("exp", 1) => x.exp(),
        ("log", 1) => x.ln() / Sym::int(10).ln(),
        ("log", 2) => x.ln() / a[1].ln(),
        ("sec", 1) => x.apply(Func::Cos).recip(),
        ("csc", 1) => x.apply(Func::Sin).recip(),
        ("cot", 1) => x.apply(Func::Tan).recip(),
        ("asec", 1) => x.recip().apply(Func::Acos),
        ("acsc", 1) => x.recip().apply(Func::Asin),
        ("acot", 1) => Sym::half() * Sym::pi() - x.apply(Func::Atan),
        ("csch", 1) => x.apply(Func::Sinh).recip(),
        ("sech", 1) => x.apply(Func::Cosh).recip(),
        ("coth", 1) => x.apply(Func::Tanh).recip(),
        ("fact", 1) => Sym::func(Func::Gamma, x + Sym::one()),
        _ => return None,
    })
}

/// Parses text and converts it structurally.
pub fn parse_sym(src: &str) -> Result<Sym, String> {
    let e = parse_expr(src)
        .map_err(|errs| errs.first().map(|e| e.message.clone()).unwrap_or_default())?;
    from_expr(&e)
}

/// The syntax tree of a symbolic expression (via its printed form).
pub fn to_expr(s: &Sym) -> Option<Expr> {
    parse_expr(&s.to_string()).ok()
}
