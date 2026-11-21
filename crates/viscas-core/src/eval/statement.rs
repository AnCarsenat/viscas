//! Statement analysis (what does a line *mean*: definition, plot relation,
//! value ...) and top-level execution.

use std::collections::BTreeSet;

use super::env::{Env, EvalOptions, canonical_name};
use super::error::{ErrorKind, EvalError, EvalResult};
use super::interp::{
    Evaluator, function_signature, is_builtin_name, split_with, with_restrictions,
};
use super::value::{FnBody, Value};
use crate::syntax::{AddOp, CmpOp, Expr, ExprKind, Program, Span, Stmt, StmtKind, Term};

/// Names that act as plot coordinates in top-level relations.
pub const PLOT_VARS: &[&str] = &["x", "y", "r", "θ", "t"];

/// What a statement is, for the plot layer and the scene.
#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    /// `a = 3`, `P = (1, 2)`, `f(x) = x^2`, `g(x, y) = ...`.
    Definition { name: String, params: Vec<String> },
    /// `y = f(x)` (also `f(x) = y` written the other way, and a bare
    /// expression in `x` such as `x^2`). [`StatementInfo::expr`] is `f`.
    ExplicitY,
    /// `x = g(y)`. [`StatementInfo::expr`] is `g`.
    ExplicitX,
    /// `r = f(θ)`. [`StatementInfo::expr`] is `f`.
    Polar,
    /// Any other equation; [`StatementInfo::expr`] is `lhs - rhs` (plot its
    /// zero set).
    Implicit,
    /// A comparison involving plot variables (`x^2 + y^2 < 4`,
    /// `y <= sin x`). [`StatementInfo::expr`] is the comparison.
    Inequality { ops: Vec<CmpOp> },
    /// A point expression with one free parameter, `(cos t, sin t)`.
    Parametric { param: String },
    /// A constant point `(1, 2)`.
    Point,
    /// Anything evaluable to a value (including comparisons without plot
    /// variables, which evaluate to booleans).
    Expression,
    /// `y1 ~ m x1 + b` — recognised, not implemented yet.
    Regression,
    /// `a -> a + 1`.
    Action,
    /// `let`, `if`, `for`, `while`, `fn`, `return`.
    Script,
}

/// Result of [`classify`].
#[derive(Clone, Debug, PartialEq)]
pub struct StatementInfo {
    pub kind: StatementKind,
    /// The expression to evaluate/plot (see [`StatementKind`]); for a
    /// definition, the right-hand side.
    pub expr: Option<Expr>,
    /// Restrictions `{...}` that apply to the whole statement.
    pub restrictions: Vec<Expr>,
    /// Names that are not bound anywhere (sorted). Plot variables show up
    /// here; anything else is an unknown name at evaluation time.
    pub free_vars: Vec<String>,
    /// Global/resolver names the statement depends on (sorted).
    pub deps: Vec<String>,
}

/// Classifies a statement against the current environment.
pub fn classify(stmt: &Stmt, env: &Env) -> StatementInfo {
    let info = |kind, expr: Option<Expr>, restrictions: &[Expr], bound: &[String]| {
        let mut fv = FreeVars::new(env);
        fv.bound.extend(bound.iter().cloned());
        if let Some(e) = &expr {
            fv.expr(e);
        }
        for r in restrictions {
            fv.expr(r);
        }
        StatementInfo {
            kind,
            expr,
            restrictions: restrictions.to_vec(),
            free_vars: fv.free.into_iter().collect(),
            deps: fv.deps.into_iter().collect(),
        }
    };
    match &stmt.kind {
        StmtKind::Equation {
            lhs,
            rhs,
            restrictions,
        } => {
            let (kind, expr, bound) = classify_equation(lhs, rhs, env);
            info(kind, Some(expr), restrictions, &bound)
        }
        StmtKind::Expr { expr, restrictions } => {
            let free = free_vars(expr, env);
            let has = |v: &str| free.iter().any(|f| f == v);
            let kind = match &*expr.kind {
                ExprKind::Compare { rest, .. } if has("x") || has("y") => {
                    StatementKind::Inequality {
                        ops: rest.iter().map(|(op, _)| *op).collect(),
                    }
                }
                ExprKind::Tuple(_) if free.is_empty() => StatementKind::Point,
                ExprKind::Tuple(_) if free.len() == 1 => StatementKind::Parametric {
                    param: free[0].clone(),
                },
                _ if free.len() == 1 && has("x") => StatementKind::ExplicitY,
                _ => StatementKind::Expression,
            };
            info(kind, Some(expr.clone()), restrictions, &[])
        }
        StmtKind::Regression {
            lhs,
            rhs,
            restrictions,
        } => {
            let e = sub(lhs, rhs);
            info(StatementKind::Regression, Some(e), restrictions, &[])
        }
        StmtKind::Action { .. } => info(StatementKind::Action, None, &[], &[]),
        _ => info(StatementKind::Script, None, &[], &[]),
    }
}

/// `(kind, expr, names bound by the definition)`.
fn classify_equation(lhs: &Expr, rhs: &Expr, env: &Env) -> (StatementKind, Expr, Vec<String>) {
    let ident = |e: &Expr| match &*e.kind {
        ExprKind::Ident(n) => Some(canonical_name(n).to_string()),
        _ => None,
    };
    let mentions = |e: &Expr, v: &str| free_vars(e, env).iter().any(|f| f == v);
    let explicit = |name: &str, other: &Expr| -> Option<StatementKind> {
        if mentions(other, name) {
            return None;
        }
        match name {
            "y" => Some(StatementKind::ExplicitY),
            "x" => Some(StatementKind::ExplicitX),
            "r" => Some(StatementKind::Polar),
            _ => None,
        }
    };
    if let Some(name) = ident(lhs) {
        if let Some(kind) = explicit(&name, rhs) {
            return (kind, rhs.clone(), vec![]);
        }
        // `xy = 1` with unknown `xy` is the implicit curve x·y = 1.
        let splits_into_coords = name.chars().count() > 1 && {
            let fv = free_vars(lhs, env);
            fv.iter().any(|v| v == "x" || v == "y")
        };
        if !matches!(name.as_str(), "x" | "y" | "r") && !splits_into_coords {
            return (
                StatementKind::Definition {
                    name,
                    params: vec![],
                },
                rhs.clone(),
                vec![],
            );
        }
    }
    if let Some(name) = ident(rhs)
        && let Some(kind) = explicit(&name, lhs)
    {
        return (kind, lhs.clone(), vec![]);
    }
    if let ExprKind::Call { callee, args } = &*lhs.kind
        && let Some((name, params)) = function_signature(callee, args)
        && !is_builtin_name(&name)
    {
        return (
            StatementKind::Definition {
                name,
                params: params.clone(),
            },
            rhs.clone(),
            params,
        );
    }
    (StatementKind::Implicit, sub(lhs, rhs), vec![])
}

/// `lhs - rhs`.
fn sub(lhs: &Expr, rhs: &Expr) -> Expr {
    Expr::new(
        ExprKind::Add(vec![
            Term {
                op: AddOp::Add,
                expr: lhs.clone(),
            },
            Term {
                op: AddOp::Sub,
                expr: rhs.clone(),
            },
        ]),
        lhs.span.to(rhs.span),
    )
}

/// Unbound names of an expression (sorted). Unknown multi-letter names are
/// split when every piece is known or a single letter (`xy` → `x`, `y`).
pub fn free_vars(e: &Expr, env: &Env) -> Vec<String> {
    let mut fv = FreeVars::new(env);
    fv.expr(e);
    fv.free.into_iter().collect()
}

struct FreeVars<'a> {
    env: &'a Env,
    bound: Vec<String>,
    free: BTreeSet<String>,
    deps: BTreeSet<String>,
}

impl<'a> FreeVars<'a> {
    fn new(env: &'a Env) -> Self {
        FreeVars {
            env,
            bound: Vec::new(),
            free: BTreeSet::new(),
            deps: BTreeSet::new(),
        }
    }

    fn known(&self, name: &str) -> bool {
        self.bound.iter().any(|b| b == name)
            || is_builtin_name(name)
            || self.env.lookup(name).is_some()
    }

    fn name(&mut self, raw: &str) {
        let name = canonical_name(raw);
        if self.bound.iter().any(|b| b == name) || is_builtin_name(name) {
            return;
        }
        if self.env.lookup(name).is_some() {
            self.deps.insert(name.to_string());
            return;
        }
        if name.starts_with("log_") {
            return;
        }
        let single = |p: &str| {
            canonical_name(p)
                .chars()
                .filter(|c| c.is_alphabetic())
                .count()
                == 1
        };
        if let Some(pieces) = split_with(name, &|p| self.known(canonical_name(p)) || single(p)) {
            for p in pieces {
                self.name(&p);
            }
            return;
        }
        self.free.insert(name.to_string());
    }

    fn with_bound(&mut self, names: &[&str], f: impl FnOnce(&mut Self)) {
        let n = self.bound.len();
        self.bound
            .extend(names.iter().map(|s| canonical_name(s).to_string()));
        f(self);
        self.bound.truncate(n);
    }

    fn expr(&mut self, e: &Expr) {
        match &*e.kind {
            ExprKind::Number { .. } | ExprKind::Str(_) => {}
            ExprKind::Ident(n) => self.name(n),
            ExprKind::Prime { expr, .. }
            | ExprKind::Unary { expr, .. }
            | ExprKind::Postfix { expr, .. } => self.expr(expr),
            ExprKind::Member { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Add(terms) => terms.iter().for_each(|t| self.expr(&t.expr)),
            ExprKind::Mul(factors) => factors.iter().for_each(|f| self.expr(&f.expr)),
            ExprKind::And(items)
            | ExprKind::Or(items)
            | ExprKind::List(items)
            | ExprKind::Tuple(items) => items.iter().for_each(|i| self.expr(i)),
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                rest.iter().for_each(|(_, e)| self.expr(e));
            }
            ExprKind::Call { callee, args } => {
                match &*callee.kind {
                    // `x(y+1)` with an unknown `x` is a product: `x` is free.
                    ExprKind::Ident(n) => self.name(n),
                    _ => self.expr(callee),
                }
                args.iter().for_each(|a| self.expr(a));
            }
            ExprKind::Index { expr, indices } => {
                self.expr(expr);
                indices.iter().for_each(|i| self.expr(i));
            }
            ExprKind::Range { start, second, end } => {
                self.expr(start);
                if let Some(s) = second {
                    self.expr(s);
                }
                self.expr(end);
            }
            ExprKind::Comprehension { body, bindings } => {
                let n = self.bound.len();
                for b in bindings {
                    self.expr(&b.iter);
                    self.bound.push(canonical_name(&b.name).to_string());
                }
                self.expr(body);
                self.bound.truncate(n);
            }
            ExprKind::Piecewise {
                branches,
                otherwise,
            } => {
                for b in branches {
                    self.expr(&b.cond);
                    if let Some(v) = &b.value {
                        self.expr(v);
                    }
                }
                if let Some(o) = otherwise {
                    self.expr(o);
                }
            }
            ExprKind::Restrict { expr, conds } => {
                self.expr(expr);
                conds.iter().for_each(|c| self.expr(c));
            }
            // `d/dx f(x)` is a function of `x`: `x` stays free.
            ExprKind::Derivative { expr, .. } => self.expr(expr),
            ExprKind::Integral { bounds, body, var } => {
                if let Some((lo, hi)) = bounds {
                    self.expr(lo);
                    self.expr(hi);
                    self.with_bound(&[var], |s| s.expr(body));
                } else {
                    self.expr(body);
                }
            }
            ExprKind::BigOp {
                var,
                from,
                to,
                body,
                ..
            } => {
                self.expr(from);
                self.expr(to);
                self.with_bound(&[var], |s| s.expr(body));
            }
        }
    }
}

/// Result of executing a top-level statement.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// An expression (or script statement) and its value.
    Value(Value),
    /// A definition was stored in the environment.
    Defined { name: String, value: Value },
    /// A plot relation (`y = x^2`, `x^2 + y^2 < 4`, ...): nothing to
    /// evaluate; the plot layer uses the analysis.
    Relation(StatementInfo),
}

/// Executes a top-level statement (console / expression-list line):
/// definitions are stored in `env`, relations are reported, expressions and
/// script statements are evaluated. In approx mode the value is converted
/// to decimals.
pub fn exec_statement(stmt: &Stmt, env: &mut Env, opts: &EvalOptions) -> EvalResult<Outcome> {
    let info = classify(stmt, env);
    match (&info.kind, &stmt.kind) {
        (
            StatementKind::Definition { name, params },
            StmtKind::Equation {
                rhs, restrictions, ..
            },
        ) => {
            let mut ev = Evaluator::new(env, opts);
            let value = if params.is_empty() {
                let body = with_restrictions(rhs, restrictions);
                let v = ev.eval(&body)?;
                finish(&mut ev, v)?
            } else {
                let body = with_restrictions(rhs, restrictions);
                ev.make_fn(name, params, FnBody::Expr(body))
            };
            ev.define_global(name, value.clone(), stmt.span)?;
            Ok(Outcome::Defined {
                name: name.clone(),
                value,
            })
        }
        (
            StatementKind::ExplicitY
            | StatementKind::ExplicitX
            | StatementKind::Polar
            | StatementKind::Implicit
            | StatementKind::Inequality { .. }
            | StatementKind::Parametric { .. },
            _,
        ) => Ok(Outcome::Relation(info)),
        (StatementKind::Regression, _) => Err(EvalError::unsupported(
            "regression (`~`) is not implemented yet",
            stmt.span,
        )),
        _ => {
            let mut ev = Evaluator::new(env, opts);
            let v = ev.exec_stmt(stmt)?.into_value();
            Ok(Outcome::Value(finish(&mut ev, v)?))
        }
    }
}

/// Approx mode: convert the final value to decimals.
fn finish(ev: &mut Evaluator<'_>, v: Value) -> EvalResult<Value> {
    if !ev.opts.approx {
        return Ok(v);
    }
    let p = ev.opts.precision;
    match v {
        Value::Number(_) | Value::List(_) | Value::Tuple(_) | Value::Matrix(_) => {
            approx_value(ev, &v, p)
        }
        other => Ok(other),
    }
}

fn approx_value(
    ev: &mut Evaluator<'_>,
    v: &Value,
    p: crate::number::Precision,
) -> EvalResult<Value> {
    match v {
        Value::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(approx_value(ev, it, p)?);
            }
            Ok(Value::List(out))
        }
        Value::Tuple(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(approx_value(ev, it, p)?);
            }
            Ok(Value::Tuple(out))
        }
        Value::Number(_) | Value::Matrix(_) => {
            ev.map_num(v, Span::default(), &|n| Ok(n.to_float(p)))
        }
        other => Ok(other.clone()),
    }
}

/// Runs a script: statements execute in a fresh local scope (`let` is
/// local; `name = value` and `fn` define globals). Returns the value of the
/// last statement (or of `return`).
pub fn run_program(program: &Program, env: &mut Env, opts: &EvalOptions) -> EvalResult<Value> {
    let mut ev = Evaluator::new(env, opts);
    ev.push_scope(Vec::new());
    let r = run_top(&mut ev, &program.stmts);
    ev.pop_scope();
    let v = r?;
    finish(&mut ev, v)
}

fn run_top(ev: &mut Evaluator<'_>, stmts: &[Stmt]) -> EvalResult<Value> {
    let mut last = Value::Undefined;
    for s in stmts {
        // Top-level `fn` defines a global even inside the script scope.
        if let StmtKind::Fn { name, params, body } = &s.kind {
            let f = ev.make_fn(name, params, FnBody::Block(body.clone()));
            ev.define_global(name, f.clone(), s.span)?;
            last = f;
            continue;
        }
        match ev.exec_stmt(s)? {
            super::interp::Flow::Value(v) => last = v,
            super::interp::Flow::Return(v) => return Ok(v),
        }
    }
    Ok(last)
}

/// Convenience bundle of an [`Env`] and [`EvalOptions`] for console-style
/// use and tests.
#[derive(Debug, Default)]
pub struct Session {
    pub env: Env,
    pub opts: EvalOptions,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses and executes `src` (one or more statements). Definitions are
    /// stored; the value of the last statement is returned (a definition
    /// returns its value). A plot relation has no value and is an error
    /// here; use [`Session::run`] to get the analysis.
    pub fn eval(&mut self, src: &str) -> EvalResult<Value> {
        let mut last = Value::Undefined;
        for outcome in self.run(src)? {
            last = match outcome {
                Outcome::Value(v) | Outcome::Defined { value: v, .. } => v,
                Outcome::Relation(info) => {
                    return Err(EvalError::new(
                        ErrorKind::Other,
                        format!(
                            "this is a relation ({:?}); it can be plotted but has no value",
                            info.kind
                        ),
                        Span::default(),
                    ));
                }
            };
        }
        Ok(last)
    }

    /// Parses and executes every statement of `src`, top-level semantics.
    /// Multi-line scripts with `let` share one scope.
    pub fn run(&mut self, src: &str) -> EvalResult<Vec<Outcome>> {
        let program = crate::syntax::parse_program(src).map_err(|e| EvalError::from_parse(&e))?;
        let scripted = program.stmts.iter().any(|s| {
            matches!(
                s.kind,
                StmtKind::Let { .. }
                    | StmtKind::If { .. }
                    | StmtKind::For { .. }
                    | StmtKind::While { .. }
                    | StmtKind::Return(_)
            )
        });
        if scripted && program.stmts.len() > 1 {
            let v = run_program(&program, &mut self.env, &self.opts)?;
            return Ok(vec![Outcome::Value(v)]);
        }
        let mut out = Vec::with_capacity(program.stmts.len());
        for s in &program.stmts {
            out.push(exec_statement(s, &mut self.env, &self.opts)?);
        }
        Ok(out)
    }

    /// Evaluates `src` and formats the result according to the session's
    /// mode (decimal output in approx mode).
    pub fn eval_to_string(&mut self, src: &str) -> EvalResult<String> {
        let v = self.eval(src)?;
        let opts = if self.opts.approx {
            super::value::FormatOptions::approx(12)
        } else {
            super::value::FormatOptions::default()
        };
        Ok(super::value::format_value(&v, &opts))
    }
}
