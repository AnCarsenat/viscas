//! `f64` fast path for plotting: [`compile_plot_fn`].
//!
//! The expression is compiled once into a small tree of `f64` operations:
//! names are resolved up front (globals and resolver values become
//! constants, user functions with expression bodies are inlined, builtins
//! become function pointers, degree mode is folded in) and constant
//! subtrees are folded. Evaluating the tree allocates nothing.
//!
//! Booleans are `1.0`/`0.0`; `NaN` means undefined (failed restriction,
//! missing piecewise branch, domain error). Anything the compiler does not
//! handle (lists, sums, integrals, derivatives, recursive or block-bodied
//! functions, complex numbers, ...) falls back to the general evaluator in
//! `float64` mode, which is correct but slower.

use std::sync::Arc;

use super::builtins;
use super::env::{AngleUnit, Env, EvalOptions, canonical_name};
use super::error::{EvalError, EvalResult};
use super::interp::{Evaluator, split_with, unknown_name};
use super::value::{FnBody, Function, Value};
use crate::number::{Number, Precision, parse_literal};
use crate::syntax::{AddOp, BinOp, CmpOp, Expr, ExprKind, MulOp, PostfixOp, UnaryOp};

/// Maximum inlining depth for user functions (deeper → fallback).
const MAX_INLINE: usize = 32;

type F1 = fn(f64) -> f64;
type F2 = fn(f64, f64) -> f64;

#[derive(Clone, Debug)]
enum Node {
    Const(f64),
    Var(usize),
    Neg(Box<Node>),
    /// Terms with "subtract" flags.
    Add(Box<[(bool, Node)]>),
    /// Factors with "divide" flags.
    Mul(Box<[(bool, Node)]>),
    Pow(Box<Node>, Box<Node>),
    PowI(Box<Node>, i32),
    /// Real `x^(p/q)` for odd `q` (negative bases allowed).
    OddRoot(Box<Node>, f64, f64),
    Mod(Box<Node>, Box<Node>),
    F1(F1, Box<Node>),
    F2(F2, Box<Node>, Box<Node>),
    Cmp(CmpOp, Box<Node>, Box<Node>),
    And(Box<[Node]>),
    Or(Box<[Node]>),
    Not(Box<Node>),
    /// Piecewise: first true condition wins, else the last node.
    Select(Box<[(Node, Node)]>, Box<Node>),
    Restrict(Box<Node>, Box<[Node]>),
}

#[inline]
fn truth(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

#[inline]
fn bool_f(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

impl Node {
    fn eval(&self, v: &[f64]) -> f64 {
        match self {
            Node::Const(c) => *c,
            Node::Var(i) => v[*i],
            Node::Neg(a) => -a.eval(v),
            Node::Add(terms) => {
                let mut acc = 0.0;
                for (neg, t) in terms.iter() {
                    let x = t.eval(v);
                    if *neg { acc -= x } else { acc += x }
                }
                acc
            }
            Node::Mul(factors) => {
                let mut acc = 1.0;
                for (div, f) in factors.iter() {
                    let x = f.eval(v);
                    if *div { acc /= x } else { acc *= x }
                }
                acc
            }
            Node::Pow(a, b) => a.eval(v).powf(b.eval(v)),
            Node::PowI(a, n) => a.eval(v).powi(*n),
            Node::OddRoot(a, p, q) => {
                let x = a.eval(v);
                let r = x.abs().powf(1.0 / q).powf(*p);
                if x < 0.0 && (*p as i64) % 2 != 0 {
                    -r
                } else {
                    r
                }
            }
            Node::Mod(a, b) => {
                let (x, y) = (a.eval(v), b.eval(v));
                x - y * (x / y).floor()
            }
            Node::F1(f, a) => f(a.eval(v)),
            Node::F2(f, a, b) => f(a.eval(v), b.eval(v)),
            Node::Cmp(op, a, b) => {
                let (x, y) = (a.eval(v), b.eval(v));
                if x.is_nan() || y.is_nan() {
                    return f64::NAN;
                }
                bool_f(match op {
                    CmpOp::Lt => x < y,
                    CmpOp::Le => x <= y,
                    CmpOp::Gt => x > y,
                    CmpOp::Ge => x >= y,
                    CmpOp::Eq => x == y,
                    CmpOp::Ne => x != y,
                })
            }
            Node::And(items) => {
                let mut undefined = false;
                for it in items.iter() {
                    let x = it.eval(v);
                    if x.is_nan() {
                        undefined = true;
                    } else if x == 0.0 {
                        return 0.0;
                    }
                }
                if undefined { f64::NAN } else { 1.0 }
            }
            Node::Or(items) => {
                let mut undefined = false;
                for it in items.iter() {
                    let x = it.eval(v);
                    if x.is_nan() {
                        undefined = true;
                    } else if x != 0.0 {
                        return 1.0;
                    }
                }
                if undefined { f64::NAN } else { 0.0 }
            }
            Node::Not(a) => {
                let x = a.eval(v);
                if x.is_nan() { x } else { bool_f(x == 0.0) }
            }
            Node::Select(branches, otherwise) => {
                for (c, val) in branches.iter() {
                    if truth(c.eval(v)) {
                        return val.eval(v);
                    }
                }
                otherwise.eval(v)
            }
            Node::Restrict(a, conds) => {
                if conds.iter().all(|c| truth(c.eval(v))) {
                    a.eval(v)
                } else {
                    f64::NAN
                }
            }
        }
    }

    fn has_var(&self) -> bool {
        match self {
            Node::Const(_) => false,
            Node::Var(_) => true,
            Node::Neg(a) | Node::PowI(a, _) | Node::OddRoot(a, _, _) | Node::F1(_, a) => {
                a.has_var()
            }
            Node::Not(a) => a.has_var(),
            Node::Add(t) | Node::Mul(t) => t.iter().any(|(_, n)| n.has_var()),
            Node::Pow(a, b) | Node::Mod(a, b) | Node::F2(_, a, b) | Node::Cmp(_, a, b) => {
                a.has_var() || b.has_var()
            }
            Node::And(items) | Node::Or(items) => items.iter().any(Node::has_var),
            Node::Select(b, o) => o.has_var() || b.iter().any(|(c, v)| c.has_var() || v.has_var()),
            Node::Restrict(a, c) => a.has_var() || c.iter().any(Node::has_var),
        }
    }

    /// Folds constant subtrees.
    fn fold(self) -> Node {
        if self.has_var() {
            self
        } else {
            Node::Const(self.eval(&[]))
        }
    }
}

enum Fail {
    /// Not handled by the compiler: use the general evaluator.
    Unsupported,
    /// A genuine error (unknown name, ...).
    Error(EvalError),
}

impl From<EvalError> for Fail {
    fn from(e: EvalError) -> Self {
        Fail::Error(e)
    }
}

type CResult = Result<Node, Fail>;

struct Compiler<'a> {
    env: &'a Env,
    vars: Vec<String>,
    /// Inlined parameters (innermost last).
    locals: Vec<(String, Node)>,
    degrees: bool,
    inline_depth: usize,
}

const DEG: f64 = std::f64::consts::PI / 180.0;

fn f64_number(n: &Number) -> Option<f64> {
    n.is_real().then(|| n.to_f64())
}

fn gamma_f(x: f64) -> f64 {
    Number::F64(x)
        .gamma(Precision::DEFAULT)
        .map_or(f64::NAN, |n| n.to_f64())
}

fn fact_f(x: f64) -> f64 {
    gamma_f(x + 1.0)
}

fn sign_f(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() {
        x
    } else {
        x.signum()
    }
}

fn frac_f(x: f64) -> f64 {
    x - x.floor()
}

fn round_f(x: f64) -> f64 {
    x.round()
}

fn log_b(x: f64, b: f64) -> f64 {
    x.ln() / b.ln()
}

fn mod_f(x: f64, y: f64) -> f64 {
    x - y * (x / y).floor()
}

fn root_f(x: f64, n: f64) -> f64 {
    if x < 0.0 && n.fract() == 0.0 && (n as i64) % 2 != 0 {
        -(-x).powf(1.0 / n)
    } else {
        x.powf(1.0 / n)
    }
}

fn recip(f: F1, x: f64) -> f64 {
    1.0 / f(x)
}

impl Compiler<'_> {
    fn lookup_local(&self, name: &str) -> Option<Node> {
        self.locals
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }

    fn is_known(&self, name: &str) -> bool {
        self.locals.iter().any(|(n, _)| n == name)
            || self.vars.iter().any(|v| v == name)
            || matches!(name, "π" | "τ" | "e" | "φ")
            || self.env.lookup(name).is_some()
    }

    fn ident(&mut self, name: &str, e: &Expr) -> CResult {
        let name = canonical_name(name);
        if let Some(n) = self.lookup_local(name) {
            return Ok(n);
        }
        if let Some(i) = self.vars.iter().position(|v| v == name) {
            return Ok(Node::Var(i));
        }
        let pi = std::f64::consts::PI;
        match name {
            "π" => return Ok(Node::Const(pi)),
            "τ" => return Ok(Node::Const(2.0 * pi)),
            "e" => return Ok(Node::Const(std::f64::consts::E)),
            "φ" => return Ok(Node::Const((1.0 + 5f64.sqrt()) / 2.0)),
            "true" => return Ok(Node::Const(1.0)),
            "false" => return Ok(Node::Const(0.0)),
            "i" | "∞" => return Err(Fail::Unsupported),
            _ => {}
        }
        if builtins::lookup(name).is_some() {
            return Err(Fail::Unsupported);
        }
        if let Some(v) = self.env.lookup(name) {
            return match v {
                Value::Number(n) => f64_number(&n).map(Node::Const).ok_or(Fail::Unsupported),
                _ => Err(Fail::Unsupported),
            };
        }
        if let Some(pieces) = split_with(name, &|p| self.is_known(canonical_name(p))) {
            let mut factors = Vec::with_capacity(pieces.len());
            for p in pieces {
                factors.push((false, self.ident(&p, e)?));
            }
            return Ok(Node::Mul(factors.into()).fold());
        }
        Err(Fail::Error(unknown_name(name, e.span)))
    }

    fn compile(&mut self, e: &Expr) -> CResult {
        let node = match &*e.kind {
            ExprKind::Number { text, imaginary } => {
                if *imaginary {
                    return Err(Fail::Unsupported);
                }
                let n = parse_literal(text).map_err(|err| EvalError::number(err, e.span))?;
                Node::Const(n.to_f64())
            }
            ExprKind::Ident(name) => self.ident(name, e)?,
            ExprKind::Unary { op, expr } => {
                let a = self.compile(expr)?;
                match op {
                    UnaryOp::Neg => Node::Neg(Box::new(a)),
                    UnaryOp::Pos => a,
                    UnaryOp::Not => Node::Not(Box::new(a)),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => match op {
                BinOp::Pow => {
                    let base = self.compile(lhs)?;
                    let exp = self.compile(rhs)?;
                    power(base, exp)
                }
                BinOp::Mod => Node::Mod(Box::new(self.compile(lhs)?), Box::new(self.compile(rhs)?)),
                BinOp::To => return Err(Fail::Unsupported),
            },
            ExprKind::Add(terms) => {
                let mut out = Vec::with_capacity(terms.len());
                for t in terms {
                    out.push((t.op == AddOp::Sub, self.compile(&t.expr)?));
                }
                Node::Add(out.into())
            }
            ExprKind::Mul(factors) => {
                let mut out = Vec::with_capacity(factors.len());
                for f in factors {
                    out.push((f.op == MulOp::Div, self.compile(&f.expr)?));
                }
                Node::Mul(out.into())
            }
            ExprKind::And(items) | ExprKind::Or(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.compile(it)?);
                }
                if matches!(&*e.kind, ExprKind::And(_)) {
                    Node::And(out.into())
                } else {
                    Node::Or(out.into())
                }
            }
            ExprKind::Postfix { op, expr } => {
                let a = self.compile(expr)?;
                match op {
                    PostfixOp::Factorial => Node::F1(fact_f, Box::new(a)),
                    PostfixOp::Percent => {
                        Node::Mul(vec![(false, a), (true, Node::Const(100.0))].into())
                    }
                }
            }
            ExprKind::Compare { first, rest } => {
                let mut lhs = self.compile(first)?;
                let mut parts = Vec::with_capacity(rest.len());
                for (op, r) in rest {
                    let rhs = self.compile(r)?;
                    parts.push(Node::Cmp(*op, Box::new(lhs), Box::new(rhs.clone())));
                    lhs = rhs;
                }
                if parts.len() == 1 {
                    parts.pop().expect("one")
                } else {
                    Node::And(parts.into())
                }
            }
            ExprKind::Call { callee, args } => self.call(callee, args)?,
            ExprKind::Piecewise {
                branches,
                otherwise,
            } => {
                let mut out = Vec::with_capacity(branches.len());
                for b in branches {
                    let c = self.compile(&b.cond)?;
                    let v = match &b.value {
                        Some(v) => self.compile(v)?,
                        None => Node::Const(1.0),
                    };
                    out.push((c, v));
                }
                let o = match otherwise {
                    Some(o) => self.compile(o)?,
                    None => Node::Const(f64::NAN),
                };
                Node::Select(out.into(), Box::new(o))
            }
            ExprKind::Restrict { expr, conds } => {
                let a = self.compile(expr)?;
                let mut cs = Vec::with_capacity(conds.len());
                for c in conds {
                    cs.push(self.compile(c)?);
                }
                Node::Restrict(Box::new(a), cs.into())
            }
            _ => return Err(Fail::Unsupported),
        };
        Ok(node.fold())
    }

    fn call(&mut self, callee: &Expr, args: &[Expr]) -> CResult {
        let ExprKind::Ident(name) = &*callee.kind else {
            return Err(Fail::Unsupported);
        };
        let name = canonical_name(name);
        // A local or plot variable followed by `(...)` is a product.
        if self.lookup_local(name).is_some() || self.vars.iter().any(|v| v == name) {
            return self.product(callee, args);
        }
        if let Some(b) = builtins::lookup(name) {
            return self.builtin(b.name, args);
        }
        match self.env.lookup(name) {
            Some(Value::Function(f)) => match &*f {
                Function::User(u) if u.captured.is_empty() => {
                    let FnBody::Expr(body) = &u.body else {
                        return Err(Fail::Unsupported);
                    };
                    if u.params.len() != args.len() || self.inline_depth >= MAX_INLINE {
                        return Err(Fail::Unsupported);
                    }
                    let mut bound = Vec::with_capacity(args.len());
                    for (p, a) in u.params.iter().zip(args) {
                        bound.push((p.clone(), self.compile(a)?));
                    }
                    // Parameters see only themselves, not the caller's locals.
                    let saved = std::mem::replace(&mut self.locals, bound);
                    self.inline_depth += 1;
                    let r = self.compile(body);
                    self.inline_depth -= 1;
                    self.locals = saved;
                    r
                }
                _ => Err(Fail::Unsupported),
            },
            Some(Value::Number(_)) => self.product(callee, args),
            _ => Err(Fail::Unsupported),
        }
    }

    fn product(&mut self, callee: &Expr, args: &[Expr]) -> CResult {
        if args.len() != 1 {
            return Err(Fail::Unsupported);
        }
        let a = self.compile(callee)?;
        let b = self.compile(&args[0])?;
        Ok(Node::Mul(vec![(false, a), (false, b)].into()))
    }

    fn builtin(&mut self, name: &str, args: &[Expr]) -> CResult {
        let deg = self.degrees;
        let one = |f: F1| -> Option<F1> { Some(f) };
        let f1: Option<F1> = match name {
            "sqrt" => one(f64::sqrt),
            "cbrt" => one(f64::cbrt),
            "abs" => one(f64::abs),
            "sign" => one(sign_f),
            "exp" => one(f64::exp),
            "ln" => one(f64::ln),
            "log" if args.len() == 1 => one(f64::log10),
            "sin" => one(f64::sin),
            "cos" => one(f64::cos),
            "tan" => one(f64::tan),
            "sec" => one(|x| recip(f64::cos, x)),
            "csc" => one(|x| recip(f64::sin, x)),
            "cot" => one(|x| x.cos() / x.sin()),
            "asin" => one(f64::asin),
            "acos" => one(f64::acos),
            "atan" => one(f64::atan),
            "asec" => one(|x| (1.0 / x).acos()),
            "acsc" => one(|x| (1.0 / x).asin()),
            "acot" => one(|x| std::f64::consts::FRAC_PI_2 - x.atan()),
            "sinh" => one(f64::sinh),
            "cosh" => one(f64::cosh),
            "tanh" => one(f64::tanh),
            "csch" => one(|x| recip(f64::sinh, x)),
            "sech" => one(|x| recip(f64::cosh, x)),
            "coth" => one(|x| recip(f64::tanh, x)),
            "asinh" => one(f64::asinh),
            "acosh" => one(f64::acosh),
            "atanh" => one(f64::atanh),
            "floor" => one(f64::floor),
            "ceil" => one(f64::ceil),
            "round" if args.len() == 1 => one(round_f),
            "trunc" => one(f64::trunc),
            "frac" => one(frac_f),
            "fact" => one(fact_f),
            "gamma" => one(gamma_f),
            _ => None,
        };
        if let Some(f) = f1 {
            if args.len() != 1 {
                return Err(Fail::Unsupported);
            }
            let mut a = self.compile(&args[0])?;
            let angle_in = matches!(name, "sin" | "cos" | "tan" | "sec" | "csc" | "cot");
            let angle_out = matches!(name, "asin" | "acos" | "atan" | "asec" | "acsc" | "acot");
            if deg && angle_in {
                a = Node::Mul(vec![(false, a), (false, Node::Const(DEG))].into());
            }
            let r = Node::F1(f, Box::new(a));
            return Ok(if deg && angle_out {
                Node::Mul(vec![(false, r), (true, Node::Const(DEG))].into())
            } else {
                r
            });
        }
        let f2: Option<F2> = match name {
            "log" => Some(log_b),
            "mod" => Some(mod_f),
            "root" => Some(root_f),
            "atan2" => Some(f64::atan2),
            _ => None,
        };
        match f2 {
            Some(f) if args.len() == 2 => {
                let a = self.compile(&args[0])?;
                let b = self.compile(&args[1])?;
                let r = Node::F2(f, Box::new(a), Box::new(b));
                Ok(if deg && name == "atan2" {
                    Node::Mul(vec![(false, r), (true, Node::Const(DEG))].into())
                } else {
                    r
                })
            }
            _ => Err(Fail::Unsupported),
        }
    }
}

/// `base^exp` with special cases for constant exponents.
fn power(base: Node, exp: Node) -> Node {
    if let Node::Const(c) = exp {
        if c.fract() == 0.0 && c.abs() < i32::MAX as f64 {
            return Node::PowI(Box::new(base), c as i32);
        }
        for q in (3..100u32).step_by(2) {
            let t = c * q as f64;
            let p = t.round();
            if (t - p).abs() < 1e-9 * t.abs().max(1.0) {
                return Node::OddRoot(Box::new(base), p, q as f64);
            }
        }
    }
    Node::Pow(Box::new(base), Box::new(exp))
}

struct Fallback {
    env: Env,
    opts: EvalOptions,
    expr: Expr,
    vars: Vec<String>,
}

enum Imp {
    Compiled(Node),
    Fallback(Arc<Fallback>),
}

/// A compiled real function of `arity` variables. `Send + Sync`; cheap to
/// call from many threads.
pub struct PlotFn {
    arity: usize,
    imp: Imp,
}

impl std::fmt::Debug for PlotFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlotFn")
            .field("arity", &self.arity)
            .field("compiled", &self.is_compiled())
            .finish()
    }
}

impl PlotFn {
    pub fn arity(&self) -> usize {
        self.arity
    }

    /// `true` if the allocation-free fast path is used (no fallback).
    pub fn is_compiled(&self) -> bool {
        matches!(self.imp, Imp::Compiled(_))
    }

    /// Evaluates at `args` (one value per variable). `NaN` where the
    /// expression is undefined or not real, or for a wrong argument count.
    pub fn eval(&self, args: &[f64]) -> f64 {
        if args.len() != self.arity {
            return f64::NAN;
        }
        match &self.imp {
            Imp::Compiled(n) => n.eval(args),
            Imp::Fallback(fb) => {
                let mut ev = Evaluator::shared(&fb.env, &fb.opts);
                let scope = fb
                    .vars
                    .iter()
                    .zip(args)
                    .map(|(n, x)| (n.clone(), Value::num(Number::F64(*x))))
                    .collect();
                ev.push_scope(scope);
                match ev.eval(&fb.expr) {
                    Ok(Value::Number(n)) if n.is_real() => n.to_f64(),
                    _ => f64::NAN,
                }
            }
        }
    }

    pub fn eval1(&self, x: f64) -> f64 {
        self.eval(&[x])
    }

    pub fn eval2(&self, x: f64, y: f64) -> f64 {
        self.eval(&[x, y])
    }

    /// As a closure of one variable.
    pub fn into_fn1(self) -> impl Fn(f64) -> f64 + Send + Sync {
        move |x| self.eval(&[x])
    }

    /// As a closure of two variables.
    pub fn into_fn2(self) -> impl Fn(f64, f64) -> f64 + Send + Sync {
        move |x, y| self.eval(&[x, y])
    }
}

/// Compiles `expr` as a real function of `vars` (e.g. `["x"]`, `["x",
/// "y"]`, `["θ"]`) against `env`. Errors for names that cannot be resolved;
/// other unsupported constructs silently use the slower general evaluator.
pub fn compile_plot_fn(
    expr: &Expr,
    vars: &[&str],
    env: &Env,
    opts: &EvalOptions,
) -> EvalResult<PlotFn> {
    let vars: Vec<String> = vars.iter().map(|v| canonical_name(v).to_string()).collect();
    let mut c = Compiler {
        env,
        vars: vars.clone(),
        locals: Vec::new(),
        degrees: opts.angle == AngleUnit::Degrees,
        inline_depth: 0,
    };
    let arity = vars.len();
    match c.compile(expr) {
        Ok(node) => Ok(PlotFn {
            arity,
            imp: Imp::Compiled(node),
        }),
        Err(Fail::Error(e)) => Err(e),
        Err(Fail::Unsupported) => {
            let mut opts = opts.clone();
            opts.float64 = true;
            Ok(PlotFn {
                arity,
                imp: Imp::Fallback(Arc::new(Fallback {
                    env: env.clone(),
                    opts,
                    expr: expr.clone(),
                    vars,
                })),
            })
        }
    }
}
