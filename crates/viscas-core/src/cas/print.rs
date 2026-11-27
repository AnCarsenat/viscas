//! Pretty printing of symbolic expressions: `x^2 + 2x + 1`, `√2/2`,
//! `π/4`, `-(x + 1)/(2y)`, `|x|`, `e^(-x)`.
//!
//! The output re-parses to the same expression: implicit multiplication
//! is only used right after a rational coefficient (`2x`, `3π`,
//! `2(x + 1)`), other factors are joined by `·`.

use std::fmt::{self, Write as _};

use num_traits::{Signed, ToPrimitive};

use super::expr::{Const, Func, Node, Sym};
use crate::number::Number;
use crate::syntax::CmpOp;

const ADD: u8 = 1;
const MUL: u8 = 2;
const POW: u8 = 3;
const ATOM: u8 = 4;

struct Printer {
    digits: usize,
}

impl fmt::Display for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text(12))
    }
}

impl Sym {
    /// Text with `digits` significant digits for inexact numbers.
    pub fn to_text(&self, digits: usize) -> String {
        let p = Printer { digits };
        let mut out = String::new();
        p.expr(self, &mut out);
        out
    }
}

fn prec(s: &Sym) -> u8 {
    match s.node() {
        Node::Add(_) | Node::Rel(..) => ADD,
        Node::Mul(_) => MUL,
        Node::Num(n) => {
            if !n.is_real() || (n.is_exact() && !n.is_integer()) {
                // `2 + 3i`, `1/2`
                if n.is_real() || n.re().is_zero() && n.im().is_integer() {
                    MUL
                } else {
                    ADD
                }
            } else if n.is_negative() {
                MUL
            } else {
                ATOM
            }
        }
        Node::Pow(_, e) => {
            if is_root(e).is_some() {
                ATOM
            } else if e.as_num().is_some_and(|n| n.is_real() && n.is_negative()) {
                MUL
            } else {
                POW
            }
        }
        _ => ATOM,
    }
}

/// `Some(n)` if the exponent is `1/n` with n = 2 or 3.
fn is_root(e: &Sym) -> Option<u32> {
    let r = e.as_rational()?;
    if r.numer().to_i64()? != 1 {
        return None;
    }
    match r.denom().to_u32()? {
        2 => Some(2),
        3 => Some(3),
        _ => None,
    }
}

/// Exponent vector key for ordering the terms of a sum.
fn degree(t: &Sym) -> (i64, bool) {
    let mut d = 0i64;
    let mut constant = true;
    for f in t.factors() {
        let (b, e) = f.base_exp();
        if b.as_var().is_some() {
            constant = false;
            if let Some(k) = e.as_i64() {
                d += k;
            } else if let Some(r) = e.as_rational() {
                d += r.floor().to_integer().to_i64().unwrap_or(0);
            }
        } else if let Node::Add(ts) = b.node() {
            let inner = ts.iter().map(|t| degree(t).0).max().unwrap_or(0);
            if !b.is_constant() {
                constant = false;
            }
            d += inner * e.as_i64().unwrap_or(1);
        } else if !b.is_constant() || !e.is_constant() {
            constant = false;
        }
    }
    (d, constant)
}

fn var_exponents(t: &Sym) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = t
        .factors()
        .iter()
        .filter_map(|f| {
            let (b, e) = f.base_exp();
            Some((b.as_var()?.to_string(), e.as_i64().unwrap_or(0)))
        })
        .collect();
    v.sort();
    v
}

/// Display order of the terms of a sum: by degree (descending), then
/// lexicographically by variables, constants last.
pub(crate) fn display_terms(ts: &[Sym]) -> Vec<Sym> {
    let mut v = ts.to_vec();
    if v.iter().all(Sym::is_constant) {
        // Constant sums: positive terms first (`4 - π`, `√6/4 - √2/4`).
        v.sort_by(|a, b| {
            is_negative_display(a)
                .cmp(&is_negative_display(b))
                .then(a.is_num().cmp(&b.is_num()))
                .then_with(|| b.cmp(a))
        });
        return v;
    }
    v.sort_by(|a, b| {
        let (da, ca) = degree(a);
        let (db, cb) = degree(b);
        let na = a.is_num();
        let nb = b.is_num();
        na.cmp(&nb)
            .then(ca.cmp(&cb))
            .then(db.cmp(&da))
            .then_with(|| {
                let (ea, eb) = (var_exponents(a), var_exponents(b));
                // x^2 before x·y before y^2: compare exponents of the first
                // variable descending.
                for (x, y) in ea.iter().zip(eb.iter()) {
                    let c = x.0.cmp(&y.0).then(y.1.cmp(&x.1));
                    if c != std::cmp::Ordering::Equal {
                        return c;
                    }
                }
                eb.len().cmp(&ea.len())
            })
            .then_with(|| a.cmp(b))
    });
    v
}

impl Printer {
    fn num(&self, n: &Number, out: &mut String) {
        if n.is_exact() {
            let _ = write!(out, "{n}");
        } else {
            out.push_str(&n.to_decimal_string(self.digits));
        }
    }

    fn wrapped(&self, s: &Sym, min: u8, out: &mut String) {
        if prec(s) < min {
            out.push('(');
            self.expr(s, out);
            out.push(')');
        } else {
            self.expr(s, out);
        }
    }

    fn expr(&self, s: &Sym, out: &mut String) {
        match s.node() {
            Node::Num(n) => self.num(n, out),
            Node::Const(Const::Pi) => out.push('π'),
            Node::Const(Const::E) => out.push('e'),
            Node::Var(v) => out.push_str(v),
            Node::Undefined => out.push_str("undefined"),
            Node::Add(ts) => self.add(ts, out),
            Node::Mul(_) => self.mul(s, out),
            Node::Pow(b, e) => self.pow(b, e, out),
            Node::Fn(Func::Abs, a) => {
                out.push('|');
                self.expr(a, out);
                out.push('|');
            }
            Node::Fn(f, a) => {
                out.push_str(f.name());
                out.push('(');
                self.expr(a, out);
                out.push(')');
            }
            Node::Rel(items, ops) => {
                for (k, it) in items.iter().enumerate() {
                    if k > 0 {
                        out.push_str(match ops[k - 1] {
                            CmpOp::Eq => " = ",
                            CmpOp::Ne => " ≠ ",
                            CmpOp::Lt => " < ",
                            CmpOp::Le => " ≤ ",
                            CmpOp::Gt => " > ",
                            CmpOp::Ge => " ≥ ",
                        });
                    }
                    self.expr(it, out);
                }
            }
        }
    }

    fn add(&self, ts: &[Sym], out: &mut String) {
        let ordered = display_terms(ts);
        for (k, t) in ordered.iter().enumerate() {
            // A complex constant prints as `a + bi` inside the sum.
            if let Some(n) = t.as_num()
                && !n.is_real()
            {
                let (re, im) = (n.re(), n.im());
                if !re.is_zero() {
                    self.signed_term(&Sym::num(re), k == 0, out);
                }
                self.signed_term(&Sym::num(im * Number::i()), k == 0 && n.re().is_zero(), out);
                continue;
            }
            self.signed_term(t, k == 0, out);
        }
    }

    fn signed_term(&self, t: &Sym, first: bool, out: &mut String) {
        let neg = is_negative_display(t);
        if first {
            if neg {
                out.push('-');
                self.wrapped(&negate_display(t), MUL, out);
            } else {
                self.wrapped(t, ADD + 1, out);
            }
        } else if neg {
            out.push_str(" - ");
            self.wrapped(&negate_display(t), MUL, out);
        } else {
            out.push_str(" + ");
            self.wrapped(t, ADD + 1, out);
        }
    }

    fn mul(&self, s: &Sym, out: &mut String) {
        if is_negative_display(s) {
            out.push('-');
            let pos = negate_display(s);
            if matches!(pos.node(), Node::Add(_)) {
                out.push('(');
                self.expr(&pos, out);
                out.push(')');
            } else {
                self.expr(&pos, out);
            }
            return;
        }
        let mut num_c = Number::one();
        let mut den_c = Number::one();
        let mut other_c: Option<Number> = None;
        let mut numer: Vec<Sym> = Vec::new();
        let mut denom: Vec<Sym> = Vec::new();
        for f in s.factors() {
            if let Some(n) = f.as_num() {
                match n.to_rational() {
                    Some(r) if n.is_real() && n.is_exact() => {
                        num_c = Number::Integer(r.numer().clone());
                        den_c = Number::Integer(r.denom().clone());
                    }
                    _ => other_c = Some(n.clone()),
                }
                continue;
            }
            let (b, e) = f.base_exp();
            if let Some(en) = e.as_num()
                && en.is_real()
                && en.is_negative()
            {
                denom.push(Sym::pow(b, Sym::num(-en)));
                continue;
            }
            if e.is_negative_term() && !e.is_num() {
                denom.push(Sym::pow(b, -&e));
                continue;
            }
            numer.push(f);
        }
        // Surds and constants first: 2√3·x, π·x.
        numer.sort_by_key(|f| match f.node() {
            Node::Pow(b, e) if b.is_num() && e.is_num() => 0,
            Node::Const(_) => 1,
            _ => 2,
        });
        let mut top = String::new();
        self.product(&num_c, other_c.as_ref(), &numer, &mut top);
        if denom.is_empty() && den_c.is_exact_one() {
            out.push_str(&top);
            return;
        }
        let top_is_sum = numer.len() == 1
            && num_c.is_exact_one()
            && other_c.is_none()
            && matches!(numer[0].node(), Node::Add(_));
        let _ = top_is_sum;
        out.push_str(&top);
        out.push('/');
        let mut bottom = String::new();
        self.product(&den_c, None, &denom, &mut bottom);
        let bottom_simple = denom.is_empty() || (den_c.is_exact_one() && denom.len() == 1);
        if bottom_simple {
            out.push_str(&bottom);
        } else {
            out.push('(');
            out.push_str(&bottom);
            out.push(')');
        }
    }

    /// `c·f1·f2…` with implicit multiplication after the coefficient.
    fn product(&self, c: &Number, other: Option<&Number>, fs: &[Sym], out: &mut String) {
        let mut wrote = false;
        if !c.is_exact_one() || (fs.is_empty() && other.is_none()) {
            self.num(c, out);
            wrote = true;
        }
        if let Some(o) = other {
            if wrote {
                out.push('·');
            }
            if o.is_real() || o.re().is_zero() {
                self.num(o, out);
            } else {
                out.push('(');
                self.num(o, out);
                out.push(')');
            }
            wrote = true;
        }
        let mut prev_coeff = wrote && other.is_none();
        for f in fs {
            let mut piece = String::new();
            self.wrapped(f, MUL + 1, &mut piece);
            if wrote {
                let starts_digit = piece.starts_with(|ch: char| ch.is_ascii_digit());
                let parens = piece.starts_with('(') && out.ends_with(')');
                if (!prev_coeff || starts_digit) && !parens {
                    out.push('·');
                }
            }
            out.push_str(&piece);
            wrote = true;
            prev_coeff = false;
        }
    }

    fn pow(&self, b: &Sym, e: &Sym, out: &mut String) {
        if let Some(n) = e.as_num()
            && n.is_real()
            && n.is_negative()
        {
            out.push_str("1/");
            let inv = Sym::pow(b.clone(), Sym::num(-n));
            self.wrapped(&inv, POW, out);
            return;
        }
        if let Some(k) = is_root(e) {
            out.push(if k == 2 { '√' } else { '∛' });
            self.wrapped(b, ATOM, out);
            return;
        }
        let base_needs = match b.node() {
            Node::Num(n) => !(n.is_real() && n.is_integer() && !n.is_negative()),
            Node::Pow(..) => true,
            _ => prec(b) < ATOM,
        };
        if base_needs {
            out.push('(');
            self.expr(b, out);
            out.push(')');
        } else {
            self.expr(b, out);
        }
        out.push('^');
        let simple = match e.node() {
            Node::Num(n) => n.is_real() && n.is_integer() && !n.is_negative(),
            Node::Var(_) | Node::Const(_) => true,
            _ => false,
        };
        if simple {
            self.expr(e, out);
        } else {
            out.push('(');
            self.expr(e, out);
            out.push(')');
        }
    }
}

/// Is the leading coefficient a negative real number?
fn is_negative_display(t: &Sym) -> bool {
    match t.node() {
        Node::Num(n) => n.is_real() && n.is_negative(),
        Node::Mul(fs) => fs[0]
            .as_num()
            .is_some_and(|n| n.is_real() && n.is_negative()),
        _ => false,
    }
}

/// `-t` without re-canonicalizing a coefficient into a sum.
fn negate_display(t: &Sym) -> Sym {
    match t.node() {
        Node::Num(n) => Sym::num(-n),
        Node::Mul(fs) => {
            let mut v = fs.clone();
            let c = -(v[0].as_num().expect("numeric coefficient"));
            if c.is_exact_one() {
                v.remove(0);
            } else {
                v[0] = Sym::num(c);
            }
            Sym::raw_mul(v)
        }
        _ => -t,
    }
}

#[allow(dead_code)]
fn is_abs_negative(n: &Number) -> bool {
    n.to_rational().is_some_and(|r| r.is_negative())
}
