//! Evaluator ↔ CAS glue: symbolic arithmetic on [`Value::Expr`],
//! symbolic elementary functions, and the CAS builtins (`simplify`,
//! `solve`, `diff`, …).
//!
//! # When values become symbolic
//!
//! - In exact mode (not `float64`), `π`, `τ`, `e` and `φ` are exact
//!   symbolic constants; `√2`, `2^(1/3)`, `sin(π/4) = √2/2`, `asin(1/2) =
//!   π/6`, `e^2` stay exact. A transcendental function of a plain rational
//!   number without a special value (`sin(1)`, `ln(2)`) is still computed
//!   numerically, as before, unless it appears inside a CAS function.
//! - Inside the arguments of a CAS builtin (*symbolic mode*), unknown
//!   names are symbols (`solve(a x + b = 0, x)`), the variable arguments
//!   (`x` in `diff(f, x)`) are always symbols, and every function stays
//!   exact (`simplify(ln 2 + ln 3)`).
//! - Arithmetic between a symbolic value and an inexact number collapses
//!   constant expressions to a number (`π + 0.5` is a float); constant
//!   expressions also become numbers wherever a number is required
//!   (ranges, indices, `round`, plotting, comparisons with `<`).

use num_traits::ToPrimitive;

use super::builtins::Builtin;
use super::env::AngleUnit;
use super::error::{ErrorKind, EvalError, EvalResult, NumSpan};
use super::interp::Evaluator;
use super::ops::Arith;
use super::value::{Function, SymValue, Value};
use crate::cas::{self, Node, Sym, convert::apply_named, solve as cs};
use crate::number::{Number, NumberError};
use crate::syntax::{CmpOp, Span};

/// Builtins that have symbolic forms.
const ELEMENTARY: &[&str] = &[
    "sqrt", "cbrt", "root", "abs", "sign", "exp", "ln", "log", "sin", "cos", "tan", "sec", "csc",
    "cot", "asin", "acos", "atan", "asec", "acsc", "acot", "sinh", "cosh", "tanh", "csch", "sech",
    "coth", "asinh", "acosh", "atanh", "floor", "ceil", "gamma", "fact",
];

const TRIG: &[&str] = &["sin", "cos", "tan", "sec", "csc", "cot"];
const INV_TRIG: &[&str] = &["asin", "acos", "atan", "asec", "acsc", "acot"];

fn contains_fn(s: &Sym) -> bool {
    matches!(s.node(), Node::Fn(..)) || s.children().iter().any(contains_fn)
}

fn contains_complex_num(s: &Sym) -> bool {
    match s.node() {
        Node::Num(n) => !n.is_real(),
        _ => s.children().iter().any(contains_complex_num),
    }
}

impl Evaluator<'_> {
    /// Numeric value of a constant symbolic value at the evaluator
    /// precision.
    pub(crate) fn expr_number(&self, s: &SymValue, span: Span) -> EvalResult<Number> {
        let sym = s.sym();
        if !sym.is_constant() {
            let vars: Vec<String> = sym.free_vars().into_iter().collect();
            return Err(EvalError::ty(
                if vars.is_empty() {
                    format!("expected a number, got `{sym}`")
                } else {
                    format!(
                        "expected a number, got an expression in {}",
                        vars.join(", ")
                    )
                },
                span,
            ));
        }
        let n = cas::approx(sym, self.opts.precision).at(span)?;
        Ok(if self.opts.float64 {
            n.into_f64_kind()
        } else {
            n
        })
    }

    /// Wraps a CAS result: undefined → error; in real mode a complex
    /// constant from real inputs → [`ErrorKind::NotReal`].
    pub(crate) fn sym_result(&self, s: Sym, inputs_real: bool, span: Span) -> EvalResult<Value> {
        if s.has_undefined() {
            return Err(EvalError::number(
                NumberError::Domain("undefined result (division by zero or a pole)"),
                span,
            ));
        }
        if !self.opts.complex && inputs_real && s.is_constant() && contains_complex_num(&s) {
            let n = cas::approx(&s, self.opts.precision).at(span)?;
            if !n.is_real() {
                return Err(EvalError::new(
                    ErrorKind::NotReal,
                    "the result is not a real number (complex mode is off)",
                    span,
                ));
            }
        }
        Ok(Value::sym(s))
    }

    /// `a op b` where at least one side is symbolic.
    pub(crate) fn sym_arith(
        &mut self,
        op: Arith,
        a: Value,
        b: Value,
        span: Span,
    ) -> EvalResult<Value> {
        let inexact = |v: &Value| matches!(v, Value::Number(n) if !n.is_exact());
        let constant = |v: &Value| match v {
            Value::Expr(s) => s.sym().is_constant(),
            _ => true,
        };
        if self.opts.float64
            || op == Arith::Mod
            || ((inexact(&a) || inexact(&b)) && constant(&a) && constant(&b))
        {
            let x = self.to_number(a, span)?;
            let y = self.to_number(b, span)?;
            return self.num_arith(op, &x, &y, span).map(Value::Number);
        }
        let (Some(x), Some(y)) = (a.as_sym(), b.as_sym()) else {
            unreachable!("sym_arith on numbers/expressions");
        };
        if x.is_rel() || y.is_rel() {
            return Err(EvalError::ty("cannot do arithmetic with a relation", span));
        }
        if matches!(op, Arith::Div) && y.is_exact_zero() {
            return Err(EvalError::number(NumberError::DivisionByZero, span));
        }
        let real = !contains_complex_num(&x) && !contains_complex_num(&y);
        let r = match op {
            Arith::Add => x + y,
            Arith::Sub => x - y,
            Arith::Mul => x * y,
            Arith::Div => x / y,
            Arith::Pow => Sym::pow(x, y),
            Arith::Mod => unreachable!(),
        };
        self.sym_result(r, real, span)
    }

    /// A number or the value of a constant expression.
    pub(crate) fn to_number(&self, v: Value, span: Span) -> EvalResult<Number> {
        match v {
            Value::Number(n) => Ok(n),
            Value::Expr(s) => self.expr_number(&s, span),
            other => Err(EvalError::ty(
                format!("expected a number, got a {}", other.type_name()),
                span,
            )),
        }
    }

    /// `a^b` for exact numbers whose value is an irrational surd
    /// (`2^(1/2) = √2`, `(-2)^(1/3) = -∛2`).
    pub(crate) fn exact_surd_pow(&self, a: &Number, b: &Number) -> Option<Value> {
        if self.opts.float64 || !a.is_exact() || !b.is_exact() || !a.is_real() || !b.is_real() {
            return None;
        }
        let br = b.to_rational()?;
        if br.is_integer() {
            return None;
        }
        if matches!(a.pow_exact(b), Ok(Some(_)) | Err(_)) {
            return None;
        }
        let den_odd = (br.denom() % 2u32).to_u32() == Some(1);
        if a.is_negative() && !den_odd {
            return None;
        }
        let s = Sym::pow(Sym::num(a.clone()), Sym::num(b.clone()));
        if s.is_num() || contains_complex_num(&s) {
            return None;
        }
        Some(Value::sym(s))
    }

    /// Symbolic evaluation of an elementary builtin, or `None` to use the
    /// numeric implementation.
    pub(crate) fn try_symbolic_call(
        &mut self,
        b: &Builtin,
        args: &[Value],
        span: Span,
    ) -> Option<EvalResult<Value>> {
        if self.opts.float64 || !ELEMENTARY.contains(&b.name) {
            return None;
        }
        if args.len() == 1
            && let Value::List(items) = &args[0]
        {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                let r = match self.try_symbolic_call(b, std::slice::from_ref(it), span) {
                    Some(r) => r,
                    None => (b.f)(self, vec![it.clone()], span),
                };
                match r {
                    Ok(v) => out.push(v),
                    Err(e) => return Some(Err(e)),
                }
            }
            return Some(Ok(Value::List(out)));
        }
        let mut syms = Vec::with_capacity(args.len());
        let mut any_expr = false;
        for a in args {
            match a {
                Value::Number(n) if n.is_exact() && n.is_real() => syms.push(Sym::num(n.clone())),
                Value::Expr(s) => {
                    if s.sym().is_rel() {
                        return None;
                    }
                    any_expr = true;
                    syms.push(s.sym().clone());
                }
                _ => return None,
            }
        }
        let degrees = self.opts.angle == AngleUnit::Degrees;
        if degrees && TRIG.contains(&b.name) {
            syms[0] = &syms[0] * Sym::pi() / Sym::int(180);
        }
        let mut r = apply_named(b.name, &syms)?;
        if degrees && INV_TRIG.contains(&b.name) {
            r = r * Sym::int(180) / Sym::pi();
        }
        if !any_expr && !self.symbolic && contains_fn(&r) {
            return None;
        }
        let real = !syms.iter().any(contains_complex_num);
        Some(self.sym_result(r, real, span))
    }

    /// Evaluates the arguments of a CAS builtin in symbolic mode, with the
    /// variable arguments bound to symbols.
    pub(crate) fn eval_symbolic_args(
        &mut self,
        b: &Builtin,
        args: &[crate::syntax::Expr],
    ) -> EvalResult<Vec<Value>> {
        use crate::syntax::ExprKind;
        let mut names: Vec<String> = Vec::new();
        for &pos in b.vars {
            let Some(a) = args.get(pos) else { continue };
            let mut add = |n: &str, always: bool| {
                let n = super::env::canonical_name(n).to_string();
                if (always || self.is_plot_or_unknown(&n)) && !names.contains(&n) {
                    names.push(n);
                }
            };
            match &*a.kind {
                ExprKind::Ident(n) => add(n, pos == 1),
                ExprKind::List(items) => {
                    for it in items {
                        if let ExprKind::Ident(n) = &*it.kind {
                            add(n, pos == 1);
                        }
                    }
                }
                _ => {}
            }
        }
        let frame = names
            .iter()
            .map(|n| (n.clone(), Value::Expr(SymValue::new(Sym::var(n)))))
            .collect();
        let saved = self.symbolic;
        self.symbolic = true;
        self.push_scope(frame);
        let mut out = Vec::with_capacity(args.len());
        let mut err = None;
        for a in args {
            match self.eval(a) {
                Ok(v) => out.push(v),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        self.pop_scope();
        self.symbolic = saved;
        match err {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    fn is_plot_or_unknown(&self, n: &str) -> bool {
        super::env::PLOT_VARS.contains(&n) || !self.is_known(n)
    }

    /// Compares values where at least one is symbolic: constant
    /// expressions numerically, others as a symbolic relation.
    pub(crate) fn sym_compare(
        &mut self,
        a: &Value,
        op: CmpOp,
        b: &Value,
        span: Span,
    ) -> EvalResult<Value> {
        let (Some(x), Some(y)) = (a.as_sym(), b.as_sym()) else {
            unreachable!("sym_compare on numbers/expressions");
        };
        if x.is_constant() && y.is_constant() {
            if matches!(op, CmpOp::Eq | CmpOp::Ne) {
                let same = x == y || {
                    let d = &x - &y;
                    d.is_exact_zero() || {
                        let p = cas::approx(&x, self.opts.precision).at(span)?;
                        let q = cas::approx(&y, self.opts.precision).at(span)?;
                        let (dr, di) = (&p - &q).to_complex_f64();
                        let (pr, pi) = p.to_complex_f64();
                        let digits = self.opts.precision.get().min(300) as i32;
                        dr.hypot(di) <= 10f64.powi(3 - digits) * pr.hypot(pi).max(1.0)
                    }
                };
                return Ok(Value::Bool(same == (op == CmpOp::Eq)));
            }
            let p = self.to_number(a.clone(), span)?;
            let q = self.to_number(b.clone(), span)?;
            return self.compare(&Value::Number(p), op, &Value::Number(q), span);
        }
        Ok(Value::Expr(SymValue::new(Sym::rel(vec![x, y], vec![op]))))
    }

    /// `-v` keeping symbolic values exact.
    pub(crate) fn sym_negate(&mut self, s: &SymValue) -> Value {
        Value::sym(-s.sym())
    }
}

// ----- CAS builtins ------------------------------------------------------------

fn arg_sym(v: &Value, span: Span) -> EvalResult<Sym> {
    v.as_sym().ok_or_else(|| {
        EvalError::ty(
            format!("expected an expression, got a {}", v.type_name()),
            span,
        )
    })
}

fn var_name(v: &Value, span: Span) -> EvalResult<String> {
    if let Value::Expr(s) = v
        && let Some(n) = s.sym().as_var()
    {
        return Ok(n.to_string());
    }
    Err(EvalError::ty(
        format!("expected a variable name, got a {}", v.type_name()),
        span,
    ))
}

fn infer_var(s: &Sym, span: Span) -> EvalResult<String> {
    let vars = s.free_vars();
    match vars.len() {
        1 => Ok(vars.into_iter().next().expect("one")),
        0 => Err(EvalError::other("the expression has no variable", span)),
        _ => Err(EvalError::other(
            format!(
                "several variables ({}); give the one to use",
                vars.into_iter().collect::<Vec<_>>().join(", ")
            ),
            span,
        )),
    }
}

fn opt_var(args: &[Value], i: usize, s: &Sym, span: Span) -> EvalResult<String> {
    match args.get(i) {
        Some(v) => var_name(v, span),
        None => infer_var(s, span),
    }
}

fn first(args: &[Value]) -> &Value {
    &args[0]
}

/// Applies `f` to the first argument, element-wise over lists.
fn map1(
    ev: &mut Evaluator<'_>,
    v: &Value,
    span: Span,
    f: &dyn Fn(&Sym) -> Sym,
) -> EvalResult<Value> {
    match v {
        Value::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(map1(ev, it, span, f)?);
            }
            Ok(Value::List(out))
        }
        Value::Undefined => Ok(Value::Undefined),
        other => {
            let s = arg_sym(other, span)?;
            ev.sym_result(f(&s), false, span)
        }
    }
}

macro_rules! cas1 {
    ($name:ident, $f:expr) => {
        pub(super) fn $name(
            ev: &mut Evaluator<'_>,
            args: Vec<Value>,
            span: Span,
        ) -> EvalResult<Value> {
            map1(ev, first(&args), span, &$f)
        }
    };
}

cas1!(b_simplify, cas::simplify);
cas1!(b_expand, cas::expand);
cas1!(b_together, cas::together);
cas1!(b_cancel, cas::cancel);
cas1!(b_numerator, cas::numerator);
cas1!(b_denominator, cas::denominator);

/// `factor(n)`: prime factors of an integer; `factor(expr)`: polynomial
/// factorization.
pub(super) fn b_factor(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    if let Value::Number(n) = first(&args) {
        let factors = n.factorize().at(span)?;
        let mut out = Vec::new();
        for (p, k) in factors {
            for _ in 0..k {
                out.push(Value::Number(Number::Integer(p.clone())));
            }
        }
        return Ok(Value::List(out));
    }
    map1(ev, first(&args), span, &cas::factor)
}

pub(super) fn b_collect(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let s = arg_sym(first(&args), span)?;
    let x = var_name(&args[1], span)?;
    ev.sym_result(cas::collect(&s, &x), false, span)
}

pub(super) fn b_coeffs(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let s = arg_sym(first(&args), span)?;
    let x = opt_var(&args, 1, &s, span)?;
    let c = cas::coeffs(&s, &x)
        .ok_or_else(|| EvalError::other(format!("not a polynomial in `{x}`"), span))?;
    let mut out = Vec::with_capacity(c.len());
    for k in c {
        out.push(ev.sym_result(k, false, span)?);
    }
    Ok(Value::List(out))
}

pub(super) fn b_degree(_ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let s = arg_sym(first(&args), span)?;
    if s.is_constant() && args.len() == 1 {
        return Ok(Value::num(Number::zero()));
    }
    let x = opt_var(&args, 1, &s, span)?;
    let d = cas::degree(&s, &x)
        .ok_or_else(|| EvalError::other(format!("not a polynomial in `{x}`"), span))?;
    Ok(Value::num(Number::from(d as u64)))
}

pub(super) fn b_partfrac(
    ev: &mut Evaluator<'_>,
    args: Vec<Value>,
    span: Span,
) -> EvalResult<Value> {
    let s = arg_sym(first(&args), span)?;
    let x = opt_var(&args, 1, &s, span)?;
    let r = cas::partfrac(&s, &x).ok_or_else(|| {
        EvalError::other(
            format!("partfrac needs a rational function of `{x}` with rational coefficients"),
            span,
        )
    })?;
    ev.sym_result(r, false, span)
}

/// A one-parameter user function applied to a symbol (`diff(f, x)`).
fn function_body(
    ev: &mut Evaluator<'_>,
    v: &Value,
    x: &str,
    span: Span,
) -> EvalResult<Option<Sym>> {
    if let Value::Function(f) = v
        && let Function::User(u) = &**f
        && u.params.len() == 1
    {
        let saved = ev.symbolic;
        ev.symbolic = true;
        let r = ev.call_function(f, vec![Value::Expr(SymValue::new(Sym::var(x)))], span);
        ev.symbolic = saved;
        return Ok(Some(arg_sym(&r?, span)?));
    }
    Ok(None)
}

pub(super) fn b_diff(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let x0 = match args.get(1) {
        Some(v) => var_name(v, span)?,
        None => match first(&args) {
            Value::Function(_) => "x".to_string(),
            v => infer_var(&arg_sym(v, span)?, span)?,
        },
    };
    let mut e = match function_body(ev, first(&args), &x0, span)? {
        Some(s) => s,
        None => arg_sym(first(&args), span)?,
    };
    // Variables and orders: diff(f, x, 2), diff(f, x, y).
    let mut plan: Vec<(String, u32)> = vec![(x0, 1)];
    for v in args.iter().skip(2) {
        match v {
            Value::Number(n) => {
                let k = n
                    .integer_value()
                    .ok()
                    .and_then(|k| k.to_u32())
                    .filter(|&k| k <= 1000)
                    .ok_or_else(|| {
                        EvalError::other("the order must be a non-negative integer", span)
                    })?;
                plan.last_mut().expect("one").1 = k;
            }
            other => plan.push((var_name(other, span)?, 1)),
        }
    }
    for (x, n) in plan {
        e = cas::diff_n(&e, &x, n).map_err(|d| EvalError::unsupported(d.0, span))?;
    }
    ev.sym_result(e, false, span)
}

fn domain_bound(ev: &Evaluator<'_>, v: &Value, span: Span) -> EvalResult<f64> {
    let n = ev.to_number(v.clone(), span)?;
    if !n.is_real() {
        return Err(EvalError::new(
            ErrorKind::NotReal,
            "bounds must be real",
            span,
        ));
    }
    Ok(n.to_f64())
}

fn solve_err(e: cs::SolveError, span: Span) -> EvalError {
    EvalError::other(e.0, span)
}

fn eq_sym(v: &Value, span: Span) -> EvalResult<Option<Sym>> {
    match v {
        Value::Bool(_) => Ok(None),
        other => arg_sym(other, span).map(Some),
    }
}

pub(super) fn b_solve(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    if let Value::List(eqs) = first(&args) {
        return solve_system(ev, eqs, args.get(1), span);
    }
    let eq = match eq_sym(first(&args), span)? {
        Some(s) => s,
        // A comparison that evaluated to true/false.
        None => {
            return Ok(match first(&args) {
                Value::Bool(true) => Value::Bool(true),
                _ => Value::List(Vec::new()),
            });
        }
    };
    let x = opt_var(&args, 1, &eq, span)?;
    let domain = match (args.get(2), args.get(3)) {
        (Some(a), Some(b)) => Some((domain_bound(ev, a, span)?, domain_bound(ev, b, span)?)),
        (Some(_), None) => {
            return Err(EvalError::other(
                "give both ends of the domain: solve(eq, x, a, b)",
                span,
            ));
        }
        _ => None,
    };
    let opts = cs::SolveOptions {
        complex: ev.opts.complex,
        domain,
    };
    match cs::solve(&eq, &x, &opts).map_err(|e| solve_err(e, span))? {
        cs::Solution::All => Ok(Value::Bool(true)),
        cs::Solution::Values(v) | cs::Solution::Intervals(v) => {
            let mut out = Vec::with_capacity(v.len());
            for s in v {
                out.push(Value::sym(s));
            }
            Ok(Value::List(out))
        }
    }
}

fn solve_system(
    ev: &mut Evaluator<'_>,
    eqs: &[Value],
    vars: Option<&Value>,
    span: Span,
) -> EvalResult<Value> {
    let mut syms = Vec::with_capacity(eqs.len());
    for e in eqs {
        match eq_sym(e, span)? {
            Some(s) => syms.push(s),
            None if matches!(e, Value::Bool(true)) => {}
            None => return Ok(Value::List(Vec::new())),
        }
    }
    let names: Vec<String> = match vars {
        Some(Value::List(vs)) => vs
            .iter()
            .map(|v| var_name(v, span))
            .collect::<EvalResult<_>>()?,
        Some(v) => vec![var_name(v, span)?],
        None => {
            let mut all = std::collections::BTreeSet::new();
            for s in &syms {
                all.extend(s.free_vars());
            }
            all.into_iter().collect()
        }
    };
    let opts = cs::SolveOptions {
        complex: ev.opts.complex,
        domain: None,
    };
    let sols = cs::solve_system(&syms, &names, &opts).map_err(|e| solve_err(e, span))?;
    let out = sols
        .into_iter()
        .map(|s| {
            Value::List(
                s.into_iter()
                    .map(|(v, e)| Value::Expr(SymValue::new(Sym::eq(Sym::var(&v), e))))
                    .collect(),
            )
        })
        .collect();
    Ok(Value::List(out))
}

pub(super) fn b_nsolve(_ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let eq = arg_sym(first(&args), span)?;
    let x = var_name(&args[1], span)?;
    let start = match (args.get(2), args.get(3)) {
        (Some(a), Some(b)) => {
            cs::NStart::Interval(domain_bound(_ev, a, span)?, domain_bound(_ev, b, span)?)
        }
        (Some(a), None) => cs::NStart::Guess(domain_bound(_ev, a, span)?),
        _ => cs::NStart::Interval(-100.0, 100.0),
    };
    let r = cs::nsolve(&eq, &x, start).map_err(|e| solve_err(e, span))?;
    let nums: Vec<Value> = r
        .into_iter()
        .map(|t| Number::f64(t).map(Value::Number).at(span))
        .collect::<EvalResult<_>>()?;
    Ok(match start {
        cs::NStart::Guess(_) => nums.into_iter().next().unwrap_or(Value::Undefined),
        _ => Value::List(nums),
    })
}

pub(super) fn b_roots(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let p = arg_sym(first(&args), span)?;
    let x = opt_var(&args, 1, &p, span)?;
    let r = cs::roots(&p, &x, ev.opts.complex).map_err(|e| solve_err(e, span))?;
    Ok(Value::List(r.into_iter().map(Value::sym).collect()))
}

/// `subs(expr, x, value)` or `subs(expr, x = value)`.
pub(super) fn b_subs(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let e = arg_sym(first(&args), span)?;
    let pairs: Vec<(String, Sym)> = match args.len() {
        3 => vec![(var_name(&args[1], span)?, arg_sym(&args[2], span)?)],
        2 => {
            let rels = match &args[1] {
                Value::List(items) => items.clone(),
                other => vec![other.clone()],
            };
            let mut out = Vec::new();
            for r in rels {
                let s = arg_sym(&r, span)?;
                match s.node() {
                    Node::Rel(items, ops) if ops.len() == 1 && ops[0] == CmpOp::Eq => {
                        let v = items[0]
                            .as_var()
                            .ok_or_else(|| EvalError::ty("expected `name = value`", span))?;
                        out.push((v.to_string(), items[1].clone()));
                    }
                    _ => return Err(EvalError::ty("expected `name = value`", span)),
                }
            }
            out
        }
        _ => {
            return Err(EvalError::new(
                ErrorKind::Arity,
                "subs(expr, x, value)",
                span,
            ));
        }
    };
    let mut r = e;
    for (v, val) in &pairs {
        r = r.subs(v, val);
    }
    ev.sym_result(r, false, span)
}

/// `steps(expr)` is evaluated by the caller (it needs the unevaluated
/// argument); this body only runs for `steps` applied to a value.
pub(super) fn b_steps(
    _ev: &mut Evaluator<'_>,
    _args: Vec<Value>,
    _span: Span,
) -> EvalResult<Value> {
    Ok(Value::List(Vec::new()))
}

impl Evaluator<'_> {
    /// `steps(expr)`: evaluates `expr` with step recording on and returns
    /// the steps as strings `rule: before → after`.
    pub(crate) fn eval_steps(
        &mut self,
        args: &[crate::syntax::Expr],
        span: Span,
    ) -> EvalResult<Value> {
        if args.len() != 1 {
            return Err(EvalError::new(
                ErrorKind::Arity,
                "`steps` takes one argument",
                span,
            ));
        }
        let (r, steps) = cas::trace(|| self.eval(&args[0]));
        r?;
        Ok(Value::List(
            steps.iter().map(|s| Value::Str(s.to_string())).collect(),
        ))
    }
}
