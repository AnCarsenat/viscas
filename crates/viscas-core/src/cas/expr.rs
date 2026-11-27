//! The symbolic expression type [`Sym`] and its automatic simplification.
//!
//! Every constructor ([`Sym::add`], [`Sym::mul`], [`Sym::pow`],
//! [`Sym::func`]) returns a *canonical* expression:
//!
//! - sums and products are flat and n-ary, sorted by the structural order,
//!   with like terms / equal bases combined (`x + x = 2x`, `x·x² = x³`);
//! - a sum has at most one numeric term and a product at most one numeric
//!   coefficient (always first); a coefficient times a single sum is
//!   distributed (`2(x + 1) = 2x + 2`);
//! - numeric powers are exact (`8^(1/3) = 2`) or normalized surds with an
//!   integer base > 1 and an exponent in (0, 1) (`√12 = 2√3`,
//!   `(1/2)^(1/2) = √2/2`);
//! - functions are evaluated at special values (`sin(π/4) = √2/2`,
//!   `ln(e) = 1`) and use their symmetries (`cos(-x) = cos(x)`);
//! - a sum/product that contains an inexact number and no symbols collapses
//!   to a number (float contagion).
//!
//! Undefined results (`1/0`, `ln(0)`, `tan(π/2)`) are [`Node::Undefined`],
//! which absorbs every operation; the evaluator reports it as an error.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use num_bigint::BigInt;
use num_integer::Integer as _;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive};

use crate::number::{Number, Precision};
use crate::syntax::CmpOp;

/// Symbolic constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Const {
    Pi,
    E,
}

/// Unary functions known to the CAS. `exp(x)` is `e^x`, `sqrt(x)` is
/// `x^(1/2)`, `sec`/`csc`/`cot` are reciprocals, `log(x, b)` is
/// `ln x / ln b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Asinh,
    Acosh,
    Atanh,
    Ln,
    Abs,
    Sign,
    Floor,
    Ceil,
    Gamma,
}

impl Func {
    pub fn name(self) -> &'static str {
        match self {
            Func::Sin => "sin",
            Func::Cos => "cos",
            Func::Tan => "tan",
            Func::Asin => "asin",
            Func::Acos => "acos",
            Func::Atan => "atan",
            Func::Sinh => "sinh",
            Func::Cosh => "cosh",
            Func::Tanh => "tanh",
            Func::Asinh => "asinh",
            Func::Acosh => "acosh",
            Func::Atanh => "atanh",
            Func::Ln => "ln",
            Func::Abs => "abs",
            Func::Sign => "sign",
            Func::Floor => "floor",
            Func::Ceil => "ceil",
            Func::Gamma => "gamma",
        }
    }

    pub fn from_name(name: &str) -> Option<Func> {
        Some(match name {
            "sin" => Func::Sin,
            "cos" => Func::Cos,
            "tan" => Func::Tan,
            "asin" => Func::Asin,
            "acos" => Func::Acos,
            "atan" => Func::Atan,
            "sinh" => Func::Sinh,
            "cosh" => Func::Cosh,
            "tanh" => Func::Tanh,
            "asinh" => Func::Asinh,
            "acosh" => Func::Acosh,
            "atanh" => Func::Atanh,
            "ln" => Func::Ln,
            "abs" => Func::Abs,
            "sign" => Func::Sign,
            "floor" => Func::Floor,
            "ceil" => Func::Ceil,
            "gamma" => Func::Gamma,
            _ => return None,
        })
    }

    /// `f(-x) = -f(x)`.
    fn is_odd(self) -> bool {
        matches!(
            self,
            Func::Sin
                | Func::Tan
                | Func::Asin
                | Func::Atan
                | Func::Sinh
                | Func::Tanh
                | Func::Asinh
                | Func::Atanh
                | Func::Sign
        )
    }

    /// `f(-x) = f(x)`.
    fn is_even(self) -> bool {
        matches!(self, Func::Cos | Func::Cosh | Func::Abs)
    }

    /// The inverse function `g` with `f(g(x)) = x` for all x.
    fn left_inverse_of(self) -> Option<Func> {
        Some(match self {
            Func::Sin => Func::Asin,
            Func::Cos => Func::Acos,
            Func::Tan => Func::Atan,
            Func::Sinh => Func::Asinh,
            Func::Cosh => Func::Acosh,
            Func::Tanh => Func::Atanh,
            _ => return None,
        })
    }
}

/// A node of a symbolic expression. Build nodes through the [`Sym`]
/// constructors, which keep them canonical.
#[derive(Debug)]
pub enum Node {
    Num(Number),
    Const(Const),
    Var(Arc<str>),
    /// At least two terms; numeric term first.
    Add(Vec<Sym>),
    /// At least two factors; numeric coefficient first.
    Mul(Vec<Sym>),
    Pow(Sym, Sym),
    Fn(Func, Sym),
    /// Relation chain `e0 op0 e1 op1 e2 …` (equations, inequalities,
    /// solution intervals `-2 < x < 2`).
    Rel(Vec<Sym>, Vec<CmpOp>),
    /// Result of an undefined operation (`1/0`); absorbs everything.
    Undefined,
}

/// A symbolic expression: a cheap-to-clone shared [`Node`].
#[derive(Clone)]
pub struct Sym(Arc<Node>);

impl fmt::Debug for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sym({self})")
    }
}

/// Precision used for numeric decisions inside the CAS (signs of constant
/// expressions, float contagion).
pub(crate) const WORK_PREC: u32 = 30;

pub(crate) fn work_prec() -> Precision {
    Precision::digits(WORK_PREC)
}

fn rat(n: &Number) -> Option<BigRational> {
    match n {
        Number::Integer(_) | Number::Rational(_) => n.to_rational(),
        _ => None,
    }
}

fn big(n: i64) -> BigInt {
    BigInt::from(n)
}

impl Sym {
    fn new(n: Node) -> Sym {
        Sym(Arc::new(n))
    }

    pub fn node(&self) -> &Node {
        &self.0
    }

    pub fn ptr_eq(&self, other: &Sym) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    // ----- atoms ---------------------------------------------------------

    pub fn num(n: Number) -> Sym {
        Sym::new(Node::Num(n))
    }

    pub fn int(n: i64) -> Sym {
        Sym::num(Number::from(n))
    }

    pub fn big_int(n: BigInt) -> Sym {
        Sym::num(Number::Integer(n))
    }

    pub fn ratio(r: BigRational) -> Sym {
        Sym::num(Number::from_ratio(r))
    }

    /// `p/q` (q ≠ 0).
    pub fn rat(p: i64, q: i64) -> Sym {
        Sym::ratio(BigRational::new(big(p), big(q)))
    }

    pub fn zero() -> Sym {
        Sym::int(0)
    }

    pub fn one() -> Sym {
        Sym::int(1)
    }

    pub fn minus_one() -> Sym {
        Sym::int(-1)
    }

    pub fn half() -> Sym {
        Sym::rat(1, 2)
    }

    pub fn i() -> Sym {
        Sym::num(Number::i())
    }

    pub fn pi() -> Sym {
        Sym::new(Node::Const(Const::Pi))
    }

    pub fn e() -> Sym {
        Sym::new(Node::Const(Const::E))
    }

    pub fn var(name: &str) -> Sym {
        Sym::new(Node::Var(Arc::from(name)))
    }

    pub fn undefined() -> Sym {
        Sym::new(Node::Undefined)
    }

    /// A relation chain (no simplification).
    pub fn rel(items: Vec<Sym>, ops: Vec<CmpOp>) -> Sym {
        debug_assert_eq!(items.len(), ops.len() + 1);
        Sym::new(Node::Rel(items, ops))
    }

    pub fn eq(lhs: Sym, rhs: Sym) -> Sym {
        Sym::rel(vec![lhs, rhs], vec![CmpOp::Eq])
    }

    /// Builds a node without any simplification (display forms such as
    /// factored results).
    pub fn raw_mul(factors: Vec<Sym>) -> Sym {
        match factors.len() {
            0 => Sym::one(),
            1 => factors.into_iter().next().expect("one"),
            _ => Sym::new(Node::Mul(factors)),
        }
    }

    pub fn raw_add(terms: Vec<Sym>) -> Sym {
        match terms.len() {
            0 => Sym::zero(),
            1 => terms.into_iter().next().expect("one"),
            _ => Sym::new(Node::Add(terms)),
        }
    }

    pub fn raw_pow(b: Sym, e: Sym) -> Sym {
        Sym::new(Node::Pow(b, e))
    }

    // ----- queries -------------------------------------------------------

    pub fn as_num(&self) -> Option<&Number> {
        match self.node() {
            Node::Num(n) => Some(n),
            _ => None,
        }
    }

    /// Exact rational value.
    pub fn as_rational(&self) -> Option<BigRational> {
        self.as_num().and_then(rat)
    }

    pub fn as_integer(&self) -> Option<BigInt> {
        self.as_rational()
            .filter(|r| r.is_integer())
            .map(|r| r.to_integer())
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_integer().and_then(|i| i.to_i64())
    }

    pub fn as_var(&self) -> Option<&str> {
        match self.node() {
            Node::Var(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_num(&self) -> bool {
        matches!(self.node(), Node::Num(_))
    }

    pub fn is_zero(&self) -> bool {
        self.as_num().is_some_and(|n| n.is_zero())
    }

    pub fn is_exact_zero(&self) -> bool {
        self.as_num().is_some_and(|n| n.is_exact_zero())
    }

    pub fn is_one(&self) -> bool {
        self.as_num().is_some_and(|n| n.is_exact_one())
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self.node(), Node::Undefined)
    }

    pub fn is_rel(&self) -> bool {
        matches!(self.node(), Node::Rel(..))
    }

    /// Children of a node (empty for atoms).
    pub fn children(&self) -> Vec<Sym> {
        match self.node() {
            Node::Add(v) | Node::Mul(v) | Node::Rel(v, _) => v.clone(),
            Node::Pow(b, e) => vec![b.clone(), e.clone()],
            Node::Fn(_, a) => vec![a.clone()],
            _ => Vec::new(),
        }
    }

    /// Rebuilds the node with mapped children (canonicalizing).
    pub fn map(&self, f: &mut dyn FnMut(&Sym) -> Sym) -> Sym {
        match self.node() {
            Node::Add(v) => Sym::add(v.iter().map(&mut *f).collect()),
            Node::Mul(v) => Sym::mul(v.iter().map(&mut *f).collect()),
            Node::Pow(b, e) => Sym::pow(f(b), f(e)),
            Node::Fn(g, a) => Sym::func(*g, f(a)),
            Node::Rel(v, ops) => Sym::rel(v.iter().map(&mut *f).collect(), ops.clone()),
            _ => self.clone(),
        }
    }

    /// Does the expression contain no variables (and is defined)?
    pub fn is_constant(&self) -> bool {
        match self.node() {
            Node::Num(_) | Node::Const(_) => true,
            Node::Var(_) | Node::Undefined | Node::Rel(..) => false,
            _ => self.children().iter().all(Sym::is_constant),
        }
    }

    pub fn has_var(&self, name: &str) -> bool {
        match self.node() {
            Node::Var(v) => &**v == name,
            Node::Num(_) | Node::Const(_) | Node::Undefined => false,
            _ => self.children().iter().any(|c| c.has_var(name)),
        }
    }

    /// Does the expression contain `sub` as a subexpression?
    pub fn contains(&self, sub: &Sym) -> bool {
        self == sub || self.children().iter().any(|c| c.contains(sub))
    }

    pub fn has_undefined(&self) -> bool {
        self.is_undefined() || self.children().iter().any(Sym::has_undefined)
    }

    /// Does the expression contain an inexact number?
    pub fn has_float(&self) -> bool {
        match self.node() {
            Node::Num(n) => !n.is_exact(),
            _ => self.children().iter().any(Sym::has_float),
        }
    }

    pub fn free_vars(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.collect_vars(&mut out);
        out
    }

    fn collect_vars(&self, out: &mut BTreeSet<String>) {
        match self.node() {
            Node::Var(v) => {
                out.insert(v.to_string());
            }
            _ => self.children().iter().for_each(|c| c.collect_vars(out)),
        }
    }

    /// Number of nodes (a complexity measure).
    pub fn size(&self) -> usize {
        1 + self.children().iter().map(Sym::size).sum::<usize>()
    }

    /// Substitutes `value` for the variable `name`.
    pub fn subs(&self, name: &str, value: &Sym) -> Sym {
        if !self.has_var(name) {
            return self.clone();
        }
        match self.node() {
            Node::Var(v) if &**v == name => value.clone(),
            _ => self.map(&mut |c| c.subs(name, value)),
        }
    }

    /// Replaces every occurrence of the subexpression `from` by `to`.
    pub fn replace(&self, from: &Sym, to: &Sym) -> Sym {
        if self == from {
            return to.clone();
        }
        if self.children().is_empty() {
            return self.clone();
        }
        self.map(&mut |c| c.replace(from, to))
    }

    /// `(numeric coefficient, rest)` of a term: `3x² → (3, x²)`.
    pub fn coeff_rest(&self) -> (Number, Sym) {
        match self.node() {
            Node::Num(n) => (n.clone(), Sym::one()),
            Node::Mul(v) => match v[0].node() {
                Node::Num(n) => {
                    let rest = if v.len() == 2 {
                        v[1].clone()
                    } else {
                        Sym::new(Node::Mul(v[1..].to_vec()))
                    };
                    (n.clone(), rest)
                }
                _ => (Number::one(), self.clone()),
            },
            _ => (Number::one(), self.clone()),
        }
    }

    /// `(base, exponent)`: `x³ → (x, 3)`, `x → (x, 1)`.
    pub fn base_exp(&self) -> (Sym, Sym) {
        match self.node() {
            Node::Pow(b, e) => (b.clone(), e.clone()),
            _ => (self.clone(), Sym::one()),
        }
    }

    /// Does the expression print with a leading minus (negative
    /// coefficient)?
    pub fn is_negative_term(&self) -> bool {
        let (c, _) = self.coeff_rest();
        c.is_real() && c.is_negative()
    }

    /// Terms of a sum (the expression itself otherwise).
    pub fn terms(&self) -> Vec<Sym> {
        match self.node() {
            Node::Add(v) => v.clone(),
            _ => vec![self.clone()],
        }
    }

    /// Factors of a product (the expression itself otherwise).
    pub fn factors(&self) -> Vec<Sym> {
        match self.node() {
            Node::Mul(v) => v.clone(),
            _ => vec![self.clone()],
        }
    }

    // ----- canonical constructors ---------------------------------------

    /// Canonical sum.
    pub fn add(terms: Vec<Sym>) -> Sym {
        let mut flat = Vec::with_capacity(terms.len());
        for t in terms {
            match t.node() {
                Node::Undefined => return t,
                Node::Add(v) => flat.extend(v.iter().cloned()),
                _ => flat.push(t),
            }
        }
        let mut constant = Number::zero();
        let mut pairs: Vec<(Sym, Number)> = Vec::with_capacity(flat.len());
        for t in flat {
            if let Node::Num(n) = t.node() {
                constant = &constant + n;
                continue;
            }
            let (c, rest) = t.coeff_rest();
            pairs.push((rest, c));
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<Sym> = Vec::with_capacity(pairs.len() + 1);
        let mut merged: Vec<(Sym, Number)> = Vec::with_capacity(pairs.len());
        for (rest, c) in pairs {
            if let Some(last) = merged.last_mut()
                && last.0 == rest
            {
                last.1 = &last.1 + &c;
                continue;
            }
            merged.push((rest, c));
        }
        let has_float = !constant.is_exact() || merged.iter().any(|(_, c)| !c.is_exact());
        if !constant.is_exact_zero() && (!constant.is_zero() || merged.is_empty()) {
            out.push(Sym::num(constant.clone()));
        }
        for (rest, c) in merged {
            if c.is_zero() {
                continue;
            }
            out.push(Sym::scale(c, rest));
        }
        let r = match out.len() {
            0 => Sym::num(if constant.is_exact() {
                Number::zero()
            } else {
                constant
            }),
            1 => out.pop().expect("one"),
            _ => Sym::new(Node::Add(out)),
        };
        if has_float { r.collapse_float() } else { r }
    }

    /// `c · rest` where `rest` has no numeric coefficient.
    fn scale(c: Number, rest: Sym) -> Sym {
        if c.is_exact_one() {
            return rest;
        }
        if rest.is_one() {
            return Sym::num(c);
        }
        let mut v = vec![Sym::num(c)];
        match rest.node() {
            Node::Mul(f) => v.extend(f.iter().cloned()),
            _ => v.push(rest),
        }
        Sym::new(Node::Mul(v))
    }

    /// If the expression is constant and contains a float, its value.
    fn collapse_float(self) -> Sym {
        if self.is_constant()
            && let Ok(n) = super::approx::approx(&self, work_prec())
        {
            return Sym::num(n);
        }
        self
    }

    /// Canonical product.
    pub fn mul(factors: Vec<Sym>) -> Sym {
        Sym::mul_depth(factors, 0)
    }

    fn mul_depth(factors: Vec<Sym>, depth: u32) -> Sym {
        let mut flat = Vec::with_capacity(factors.len());
        for f in factors {
            match f.node() {
                Node::Undefined => return f,
                Node::Mul(v) => flat.extend(v.iter().cloned()),
                _ => flat.push(f),
            }
        }
        let mut coeff = Number::one();
        let mut pairs: Vec<(Sym, Sym)> = Vec::with_capacity(flat.len());
        for f in flat {
            if let Node::Num(n) = f.node() {
                coeff = &coeff * n;
                continue;
            }
            pairs.push(f.base_exp());
        }
        if coeff.is_exact_zero() {
            return Sym::zero();
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut merged: Vec<(Sym, Vec<Sym>)> = Vec::with_capacity(pairs.len());
        for (b, e) in pairs {
            if let Some(last) = merged.last_mut()
                && last.0 == b
            {
                last.1.push(e);
                continue;
            }
            merged.push((b, vec![e]));
        }
        // Rebuild powers; numeric results go into the coefficient.
        let mut out: Vec<Sym> = Vec::with_capacity(merged.len());
        let mut again = false;
        for (b, es) in merged {
            let e = if es.len() == 1 {
                es.into_iter().next().expect("one")
            } else {
                Sym::add(es)
            };
            let p = Sym::pow(b, e);
            match p.node() {
                Node::Undefined => return p,
                Node::Num(n) => coeff = &coeff * n,
                Node::Mul(_) => {
                    again = true;
                    out.push(p);
                }
                _ => out.push(p),
            }
        }
        if coeff.is_exact_zero() {
            return Sym::zero();
        }
        // Combine numeric surds with the same exponent: √2·√3 = √6.
        let mut surds: Vec<(BigRational, BigInt)> = Vec::new();
        out.retain(|f| {
            if let Node::Pow(b, e) = f.node()
                && let (Some(bi), Some(er)) = (b.as_integer(), e.as_rational())
            {
                if let Some(s) = surds.iter_mut().find(|s| s.0 == er) {
                    s.1 *= bi;
                    again = true;
                } else {
                    surds.push((er, bi));
                }
                return false;
            }
            true
        });
        for (e, b) in surds {
            out.push(Sym::pow(Sym::big_int(b), Sym::ratio(e)));
        }
        if again && depth < 8 {
            let mut all = vec![Sym::num(coeff)];
            all.extend(out);
            return Sym::mul_depth(all, depth + 1);
        }
        let has_float = !coeff.is_exact();
        out.sort();
        let r = if out.is_empty() {
            Sym::num(coeff)
        } else if out.len() == 1 && coeff.is_exact_one() {
            out.pop().expect("one")
        } else if out.len() == 1 && matches!(out[0].node(), Node::Add(_)) && coeff.is_exact() {
            // Distribute a coefficient over a single sum.
            let terms = out[0]
                .terms()
                .into_iter()
                .map(|t| Sym::mul(vec![Sym::num(coeff.clone()), t]))
                .collect();
            return Sym::add(terms);
        } else {
            let mut v = Vec::with_capacity(out.len() + 1);
            if !coeff.is_exact_one() {
                v.push(Sym::num(coeff));
            }
            v.extend(out);
            if v.len() == 1 {
                v.pop().expect("one")
            } else {
                Sym::new(Node::Mul(v))
            }
        };
        if has_float { r.collapse_float() } else { r }
    }

    /// Canonical power.
    pub fn pow(b: Sym, e: Sym) -> Sym {
        if b.is_undefined() {
            return b;
        }
        if e.is_undefined() {
            return e;
        }
        if e.is_exact_zero() {
            return Sym::one();
        }
        if e.is_one() {
            return b;
        }
        if b.is_one() {
            return Sym::one();
        }
        if let (Node::Num(x), Node::Num(y)) = (b.node(), e.node()) {
            return num_pow(x, y);
        }
        if b.is_exact_zero() {
            return match e.as_num() {
                Some(n) if n.is_real() && n.is_negative() => Sym::undefined(),
                Some(_) => Sym::zero(),
                None if e.is_constant() => match super::approx::sign(&e) {
                    Some(Ordering::Greater) => Sym::zero(),
                    Some(Ordering::Less) => Sym::undefined(),
                    _ => Sym::raw_pow(b, e),
                },
                None => Sym::raw_pow(b, e),
            };
        }
        let e_int = e.as_integer();
        match b.node() {
            Node::Pow(bb, be) => {
                // (x^a)^b = x^(ab) for integer b, or a ∈ (0, 1].
                let ok = e_int.is_some()
                    || be.as_rational().is_some_and(|a| {
                        a.is_positive() && a <= BigRational::one() && e.as_rational().is_some()
                    });
                if ok {
                    return Sym::pow(bb.clone(), Sym::mul(vec![be.clone(), e]));
                }
            }
            Node::Mul(fs) if e_int.is_some() => {
                return Sym::mul(fs.iter().map(|f| Sym::pow(f.clone(), e.clone())).collect());
            }
            Node::Mul(fs) if e.as_rational().is_some() => {
                // Pull out positive numeric coefficients: (4x)^(1/2) = 2·x^(1/2).
                if let Some(c) = fs[0].as_rational()
                    && c.is_positive()
                {
                    let rest = Sym::mul(fs[1..].to_vec());
                    return Sym::mul(vec![Sym::pow(fs[0].clone(), e.clone()), Sym::pow(rest, e)]);
                }
            }
            Node::Const(Const::E) => {
                if let Some(v) = exp_special(&e) {
                    return v;
                }
            }
            // |u|^(2k) = u^(2k) (real u).
            Node::Fn(Func::Abs, u) if e_int.as_ref().is_some_and(|k| k.is_even()) => {
                return Sym::pow(u.clone(), e);
            }
            _ => {}
        }
        if (b.has_float() || e.has_float())
            && b.is_constant()
            && e.is_constant()
            && let Ok(n) = super::approx::approx(&Sym::raw_pow(b.clone(), e.clone()), work_prec())
        {
            return Sym::num(n);
        }
        Sym::raw_pow(b, e)
    }

    /// Canonical function application.
    pub fn func(f: Func, a: Sym) -> Sym {
        if a.is_undefined() {
            return a;
        }
        if a.is_constant() && a.has_float() {
            return match super::approx::approx(&Sym::new(Node::Fn(f, a)), work_prec()) {
                Ok(n) => Sym::num(n),
                Err(_) => Sym::undefined(),
            };
        }
        if let Some(v) = super::special::func_special(f, &a) {
            return v;
        }
        // Symmetry: f(-x) = ±f(x).
        if a.is_negative_term() && (f.is_odd() || f.is_even()) {
            let pos = -&a;
            let v = Sym::func(f, pos);
            return if f.is_odd() { -&v } else { v };
        }
        // f(g(x)) = x
        if let Node::Fn(g, inner) = a.node()
            && f.left_inverse_of() == Some(*g)
        {
            return inner.clone();
        }
        Sym::new(Node::Fn(f, a))
    }

    // ----- convenience ---------------------------------------------------

    pub fn sqrt(&self) -> Sym {
        Sym::pow(self.clone(), Sym::half())
    }

    pub fn exp(&self) -> Sym {
        Sym::pow(Sym::e(), self.clone())
    }

    pub fn ln(&self) -> Sym {
        Sym::func(Func::Ln, self.clone())
    }

    pub fn recip(&self) -> Sym {
        Sym::pow(self.clone(), Sym::minus_one())
    }

    pub fn powi(&self, n: i64) -> Sym {
        Sym::pow(self.clone(), Sym::int(n))
    }

    pub fn apply(&self, f: Func) -> Sym {
        Sym::func(f, self.clone())
    }
}

/// `e^(c·iπ)` and `e^(ln x)` special values.
fn exp_special(e: &Sym) -> Option<Sym> {
    if let Node::Fn(Func::Ln, x) = e.node() {
        return Some(x.clone());
    }
    let (c, rest) = e.coeff_rest();
    if let Node::Fn(Func::Ln, x) = rest.node()
        && c.is_integer()
    {
        return Some(Sym::pow(x.clone(), Sym::num(c)));
    }
    // e^(q·i·π) with rational q = cos(qπ) + i sin(qπ) when exact.
    if let Node::Mul(fs) = e.node()
        && fs.len() == 2
        && matches!(fs[1].node(), Node::Const(Const::Pi))
        && let Some(n) = fs[0].as_num()
        && n.is_complex()
        && n.re().is_exact_zero()
        && let Some(q) = rat(&n.im())
    {
        let angle = Sym::mul(vec![Sym::ratio(q), Sym::pi()]);
        let c = super::special::func_special(Func::Cos, &angle)?;
        let s = super::special::func_special(Func::Sin, &angle)?;
        if c.is_num() && s.is_num() {
            return Some(c + Sym::i() * s);
        }
    }
    None
}

/// `x^y` for numbers: exact when possible, normalized surd otherwise.
fn num_pow(x: &Number, y: &Number) -> Sym {
    if !x.is_exact() || !y.is_exact() {
        return match x.pow(y, work_prec()) {
            Ok(n) => Sym::num(n),
            Err(_) => Sym::undefined(),
        };
    }
    match x.pow_exact(y) {
        Ok(Some(n)) => return Sym::num(n),
        Err(_) => return Sym::undefined(),
        Ok(None) => {}
    }
    let (Some(b), Some(e)) = (rat(x), rat(y)) else {
        return Sym::raw_pow(Sym::num(x.clone()), Sym::num(y.clone()));
    };
    let q = e.denom().clone();
    let Some(qu) = q.to_u32().filter(|&q| q <= 64) else {
        return Sym::raw_pow(Sym::num(x.clone()), Sym::num(y.clone()));
    };
    if b.is_negative() {
        let abs = Sym::pow(Sym::ratio(-b), Sym::ratio(e.clone()));
        let p = e.numer();
        if qu % 2 == 1 {
            // Real odd root: (-a)^(p/q) = ±a^(p/q).
            return if p.is_odd() { -&abs } else { abs };
        }
        if qu == 2 {
            let k = p.mod_floor(&big(4)).to_u32().unwrap_or(0);
            let unit = match k {
                0 => Sym::one(),
                1 => Sym::i(),
                2 => Sym::minus_one(),
                _ => -&Sym::i(),
            };
            return unit * abs;
        }
        return Sym::raw_pow(Sym::num(x.clone()), Sym::num(y.clone()));
    }
    // b > 0: b^(k + a/q) = b^k · (n^a d^(q-a))^(1/q) / d
    let k = e.floor();
    let frac = &e - &k;
    let a = frac.numer().clone();
    let Some(au) = a.to_u32() else {
        return Sym::raw_pow(Sym::num(x.clone()), Sym::num(y.clone()));
    };
    let (n, d) = (b.numer().clone(), b.denom().clone());
    let bk = match Number::from_ratio(b.clone()).pow_exact(&Number::from_ratio(k)) {
        Ok(Some(v)) => v,
        _ => return Sym::raw_pow(Sym::num(x.clone()), Sym::num(y.clone())),
    };
    let m = num_traits::pow(n, au as usize) * num_traits::pow(d.clone(), (qu - au) as usize);
    let (outside, inside) = extract_power(&m, qu);
    let coeff = Number::from_ratio(BigRational::new(outside, d)) * bk;
    if inside.is_one() {
        return Sym::num(coeff);
    }
    // Reduce the root index: 4^(1/4) = 2^(1/2).
    let (base, idx) = reduce_root(inside, qu);
    let surd = Sym::raw_pow(Sym::big_int(base), Sym::rat(1, idx as i64));
    if coeff.is_exact_one() {
        surd
    } else {
        Sym::new(Node::Mul(vec![Sym::num(coeff), surd]))
    }
}

/// `m = s^q · t` with `t` free of q-th powers: returns `(s, t)`.
fn extract_power(m: &BigInt, q: u32) -> (BigInt, BigInt) {
    let mut s = BigInt::one();
    let mut t = BigInt::one();
    match crate::number::factorize(m) {
        Ok(fs) => {
            for (p, k) in fs {
                s *= num_traits::pow(p.clone(), (k / q) as usize);
                t *= num_traits::pow(p, (k % q) as usize);
            }
            (s, t)
        }
        Err(_) => (BigInt::one(), m.clone()),
    }
}

/// `t^(1/q)` with `t = u^g`: `(u, q/g)` for the largest such g dividing q.
fn reduce_root(t: BigInt, q: u32) -> (BigInt, u32) {
    let Ok(fs) = crate::number::factorize(&t) else {
        return (t, q);
    };
    let mut g = q;
    for (_, k) in &fs {
        g = g.gcd(k);
    }
    if g <= 1 {
        return (t, q);
    }
    let mut u = BigInt::one();
    for (p, k) in fs {
        u *= num_traits::pow(p, (k / g) as usize);
    }
    (u, q / g)
}

// ----- ordering ------------------------------------------------------------

fn rank(n: &Node) -> u8 {
    match n {
        Node::Num(_) => 0,
        Node::Const(_) => 1,
        Node::Var(_) => 2,
        Node::Pow(..) => 3,
        Node::Mul(_) => 4,
        Node::Add(_) => 5,
        Node::Fn(..) => 6,
        Node::Rel(..) => 7,
        Node::Undefined => 8,
    }
}

fn cmp_num(a: &Number, b: &Number) -> Ordering {
    let key = |n: &Number| (n.re(), n.im());
    let (ar, ai) = key(a);
    let (br, bi) = key(b);
    ar.cmp_real(&br)
        .unwrap_or(Ordering::Equal)
        .then_with(|| ai.cmp_real(&bi).unwrap_or(Ordering::Equal))
        .then_with(|| b.is_exact().cmp(&a.is_exact()))
}

fn cmp_slices(a: &[Sym], b: &[Sym]) -> Ordering {
    // Compare from the most significant (last) element, like polynomials.
    for (x, y) in a.iter().rev().zip(b.iter().rev()) {
        let c = x.cmp(y);
        if c != Ordering::Equal {
            return c;
        }
    }
    a.len().cmp(&b.len())
}

impl Ord for Sym {
    /// A total order designed so that sums sort by degree: a product is
    /// compared as its list of factors (most significant last), a factor
    /// as `(base, exponent)` (`x` is `x^1`).
    fn cmp(&self, other: &Sym) -> Ordering {
        if self.ptr_eq(other) {
            return Ordering::Equal;
        }
        let va = mul_view(self);
        let vb = mul_view(other);
        if va.len() == 1 && vb.len() == 1 {
            return cmp_factor(&va[0], &vb[0]);
        }
        for (x, y) in va.iter().rev().zip(vb.iter().rev()) {
            let c = cmp_factor(x, y);
            if c != Ordering::Equal {
                return c;
            }
        }
        va.len().cmp(&vb.len())
    }
}

fn mul_view(s: &Sym) -> &[Sym] {
    match s.node() {
        Node::Mul(v) => v,
        _ => std::slice::from_ref(s),
    }
}

fn cmp_factor(a: &Sym, b: &Sym) -> Ordering {
    let one = Sym::one();
    let (ba, ea) = match a.node() {
        Node::Pow(x, e) => (x, e),
        _ => (a, &one),
    };
    let (bb, eb) = match b.node() {
        Node::Pow(x, e) => (x, e),
        _ => (b, &one),
    };
    cmp_atom(ba, bb).then_with(|| ea.cmp(eb))
}

fn cmp_atom(a: &Sym, b: &Sym) -> Ordering {
    let (x, y) = (a.node(), b.node());
    let r = rank(x).cmp(&rank(y));
    if r != Ordering::Equal {
        return r;
    }
    match (x, y) {
        (Node::Num(p), Node::Num(q)) => cmp_num(p, q),
        (Node::Const(p), Node::Const(q)) => p.cmp(q),
        (Node::Var(p), Node::Var(q)) => p.cmp(q),
        (Node::Pow(b1, e1), Node::Pow(b2, e2)) => b1.cmp(b2).then_with(|| e1.cmp(e2)),
        (Node::Mul(p), Node::Mul(q)) | (Node::Add(p), Node::Add(q)) => cmp_slices(p, q),
        (Node::Fn(f, p), Node::Fn(g, q)) => f.cmp(g).then_with(|| p.cmp(q)),
        (Node::Rel(p, o1), Node::Rel(q, o2)) => cmp_slices(p, q).then_with(|| {
            let k = |o: &CmpOp| o.as_str();
            o1.iter().map(k).cmp(o2.iter().map(k))
        }),
        _ => Ordering::Equal,
    }
}

impl PartialOrd for Sym {
    fn partial_cmp(&self, other: &Sym) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Sym {
    fn eq(&self, other: &Sym) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        match (self.node(), other.node()) {
            (Node::Num(a), Node::Num(b)) => a.is_exact() == b.is_exact() && a == b,
            (Node::Const(a), Node::Const(b)) => a == b,
            (Node::Var(a), Node::Var(b)) => a == b,
            (Node::Add(a), Node::Add(b)) | (Node::Mul(a), Node::Mul(b)) => a == b,
            (Node::Pow(a, b), Node::Pow(c, d)) => a == c && b == d,
            (Node::Fn(f, a), Node::Fn(g, b)) => f == g && a == b,
            (Node::Rel(a, o), Node::Rel(b, p)) => a == b && o == p,
            (Node::Undefined, Node::Undefined) => true,
            _ => false,
        }
    }
}

impl Eq for Sym {}

// ----- operators -------------------------------------------------------------

macro_rules! sym_ops {
    ($tr:ident, $m:ident, |$a:ident, $b:ident| $body:expr) => {
        impl std::ops::$tr<Sym> for Sym {
            type Output = Sym;
            fn $m(self, rhs: Sym) -> Sym {
                let ($a, $b) = (self, rhs);
                $body
            }
        }
        impl std::ops::$tr<&Sym> for &Sym {
            type Output = Sym;
            fn $m(self, rhs: &Sym) -> Sym {
                let ($a, $b) = (self.clone(), rhs.clone());
                $body
            }
        }
        impl std::ops::$tr<&Sym> for Sym {
            type Output = Sym;
            fn $m(self, rhs: &Sym) -> Sym {
                let ($a, $b) = (self, rhs.clone());
                $body
            }
        }
        impl std::ops::$tr<Sym> for &Sym {
            type Output = Sym;
            fn $m(self, rhs: Sym) -> Sym {
                let ($a, $b) = (self.clone(), rhs);
                $body
            }
        }
    };
}

sym_ops!(Add, add, |a, b| Sym::add(vec![a, b]));
sym_ops!(Sub, sub, |a, b| Sym::add(vec![
    a,
    Sym::mul(vec![Sym::minus_one(), b])
]));
sym_ops!(Mul, mul, |a, b| Sym::mul(vec![a, b]));
sym_ops!(Div, div, |a, b| Sym::mul(vec![
    a,
    Sym::pow(b, Sym::minus_one())
]));

impl std::ops::Neg for Sym {
    type Output = Sym;
    fn neg(self) -> Sym {
        Sym::mul(vec![Sym::minus_one(), self])
    }
}

impl std::ops::Neg for &Sym {
    type Output = Sym;
    fn neg(self) -> Sym {
        Sym::mul(vec![Sym::minus_one(), self.clone()])
    }
}

impl From<i64> for Sym {
    fn from(n: i64) -> Sym {
        Sym::int(n)
    }
}

impl From<Number> for Sym {
    fn from(n: Number) -> Sym {
        Sym::num(n)
    }
}
