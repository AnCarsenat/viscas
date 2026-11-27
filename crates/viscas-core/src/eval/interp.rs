//! The tree-walking evaluator: name resolution, expressions, calls and
//! statements.

use std::sync::Arc;
use std::sync::atomic::Ordering as AtomicOrdering;

use super::builtins;
use super::env::{CONSTANTS, Env, EvalOptions, canonical_name, is_reserved};
use super::error::{ErrorKind, EvalError, EvalResult, NumSpan};
use super::ops::Arith;
use super::value::{FnBody, Function, Matrix, UserFn, Value};
use crate::number::{InterruptGuard, Number, install_interrupt, parse_literal};
use crate::syntax::{
    AddOp, BinOp, Block, Branch, ElseBranch, Expr, ExprKind, Factor, MulOp, PostfixOp, Span, Stmt,
    StmtKind, Term, UnaryOp,
};

/// Longest identifier the evaluator tries to split into a product.
const MAX_SPLIT_LEN: usize = 24;

/// Maximum number of elements a range or comprehension may produce.
pub(crate) const MAX_LIST_LEN: usize = 1_000_000;

pub(crate) enum EnvRef<'a> {
    Mut(&'a mut Env),
    Shared(&'a Env),
}

impl EnvRef<'_> {
    fn get(&self) -> &Env {
        match self {
            EnvRef::Mut(e) => e,
            EnvRef::Shared(e) => e,
        }
    }
}

/// Control flow of statement execution.
pub(crate) enum Flow {
    Value(Value),
    Return(Value),
}

impl Flow {
    pub(crate) fn into_value(self) -> Value {
        match self {
            Flow::Value(v) | Flow::Return(v) => v,
        }
    }
}

/// Tree-walking evaluator. Create one per evaluation request; it holds the
/// lexical scopes, the step/depth counters and the interrupt guard.
pub struct Evaluator<'a> {
    env: EnvRef<'a>,
    pub(crate) opts: EvalOptions,
    /// Lexical scopes (innermost last). Only `scopes[base..]` is visible.
    scopes: Vec<Vec<(String, Value)>>,
    /// First scope of the current function frame.
    base: usize,
    depth: usize,
    steps: u64,
    /// Stack address when the evaluator was created (stack budget origin).
    stack_base: usize,
    /// List elements created so far (ranges, comprehensions).
    elements: u64,
    /// Captured variables of the closure being executed (shared).
    captured: Arc<Vec<(String, Value)>>,
    pi: Option<Number>,
    /// Symbolic mode (arguments of CAS functions): unknown names are
    /// symbols and functions stay exact.
    pub(crate) symbolic: bool,
    _guard: Option<InterruptGuard>,
}

impl<'a> Evaluator<'a> {
    /// Evaluator that may define globals (scripts, console).
    pub fn new(env: &'a mut Env, opts: &EvalOptions) -> Self {
        Self::with_env(EnvRef::Mut(env), opts)
    }

    /// Read-only evaluator: definitions fail.
    pub fn shared(env: &'a Env, opts: &EvalOptions) -> Self {
        Self::with_env(EnvRef::Shared(env), opts)
    }

    fn with_env(env: EnvRef<'a>, opts: &EvalOptions) -> Self {
        let guard = opts.interrupt.clone().map(install_interrupt);
        let probe = 0u8;
        let stack_base = std::hint::black_box(&probe) as *const u8 as usize;
        Evaluator {
            env,
            opts: opts.clone(),
            scopes: Vec::new(),
            base: 0,
            depth: 0,
            steps: 0,
            stack_base,
            elements: 0,
            captured: Arc::new(Vec::new()),
            pi: None,
            symbolic: false,
            _guard: guard,
        }
    }

    pub(crate) fn env(&self) -> &Env {
        self.env.get()
    }

    // ----- budgets -------------------------------------------------------

    /// Counts one evaluation step; polls the interrupt flag periodically.
    /// Accounts for `n` list elements created by ranges and comprehensions
    /// (one global budget per evaluator) and charges `n` steps.
    pub(crate) fn alloc(&mut self, n: u64, span: Span) -> EvalResult<()> {
        self.elements = self.elements.saturating_add(n);
        if self.elements > self.opts.max_elements {
            return Err(EvalError::new(
                ErrorKind::Other,
                format!(
                    "too many list elements created (limit {})",
                    self.opts.max_elements
                ),
                span,
            ));
        }
        self.charge(n, span)
    }

    #[inline]
    pub(crate) fn tick(&mut self, span: Span) -> EvalResult<()> {
        self.charge(1, span)
    }

    /// Charges `n` steps of work (vectorised list operations, matrix
    /// products and ranges charge proportionally to their size) and polls
    /// the interrupt flag every 1024 steps.
    pub(crate) fn charge(&mut self, n: u64, span: Span) -> EvalResult<()> {
        let before = self.steps;
        self.steps = self.steps.saturating_add(n);
        if self.steps > self.opts.max_steps {
            return Err(EvalError::new(
                ErrorKind::StepLimit,
                format!(
                    "evaluation step limit ({}) exceeded — infinite loop?",
                    self.opts.max_steps
                ),
                span,
            ));
        }
        if (before >> 10) != (self.steps >> 10)
            && let Some(flag) = &self.opts.interrupt
            && flag.load(AtomicOrdering::Relaxed)
        {
            return Err(EvalError::new(
                ErrorKind::Interrupted,
                "computation interrupted",
                span,
            ));
        }
        Ok(())
    }

    /// Enters one nesting level: checks the depth limit and the stack
    /// budget (bytes of machine stack used since the evaluator was created),
    /// so deep recursion is an error instead of a stack overflow.
    fn enter(&mut self, span: Span) -> EvalResult<()> {
        let probe = 0u8;
        let here = std::hint::black_box(&probe) as *const u8 as usize;
        let used = self.stack_base.saturating_sub(here);
        if self.depth >= self.opts.max_depth || used > self.opts.max_stack {
            return Err(EvalError::new(
                ErrorKind::DepthLimit,
                format!(
                    "nesting/recursion limit exceeded (depth {}, stack {} KiB)",
                    self.depth,
                    used / 1024
                ),
                span,
            ));
        }
        self.depth += 1;
        Ok(())
    }

    // ----- scopes --------------------------------------------------------

    pub(crate) fn push_scope(&mut self, vars: Vec<(String, Value)>) {
        self.scopes.push(vars);
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Pops the innermost scope and returns its variables.
    pub(crate) fn take_scope(&mut self) -> Vec<(String, Value)> {
        self.scopes.pop().unwrap_or_default()
    }

    /// Binds `name` in the innermost scope (creating one if needed).
    pub(crate) fn bind_local(&mut self, name: &str, value: Value) {
        if self.scopes.len() <= self.base {
            self.scopes.push(Vec::new());
        }
        let scope = self.scopes.last_mut().expect("scope");
        let name = canonical_name(name);
        if let Some(slot) = scope.iter_mut().find(|(n, _)| n == name) {
            slot.1 = value;
        } else {
            scope.push((name.to_string(), value));
        }
    }

    /// Replaces the innermost binding of `name`; `false` if none exists.
    pub(crate) fn set_local(&mut self, name: &str, value: Value) -> bool {
        let base = self.base;
        for scope in self.scopes[base..].iter_mut().rev() {
            if let Some(slot) = scope.iter_mut().rev().find(|(n, _)| n == name) {
                slot.1 = value;
                return true;
            }
        }
        false
    }

    pub(crate) fn has_local_scope(&self) -> bool {
        self.scopes.len() > self.base
    }

    fn lookup_local(&self, name: &str) -> Option<&Value> {
        self.scopes[self.base..]
            .iter()
            .rev()
            .find_map(|s| s.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v))
            .or_else(|| {
                self.captured
                    .iter()
                    .rev()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v)
            })
    }

    /// Captured variables of the current closure followed by its locals
    /// (later entries shadow earlier ones).
    fn visible_locals(&self) -> Vec<(String, Value)> {
        self.captured
            .iter()
            .chain(self.scopes[self.base..].iter().flatten())
            .cloned()
            .collect()
    }

    /// Names of the visible locals (for statement analysis).
    pub(crate) fn local_names(&self) -> Vec<String> {
        self.visible_locals().into_iter().map(|(n, _)| n).collect()
    }

    /// Defines a global (fails in a read-only evaluator or for reserved
    /// names).
    pub(crate) fn define_global(&mut self, name: &str, value: Value, span: Span) -> EvalResult<()> {
        match &mut self.env {
            EnvRef::Mut(env) => env
                .define(name, value)
                .map_err(|m| EvalError::new(ErrorKind::Definition, m, span)),
            EnvRef::Shared(_) => Err(EvalError::new(
                ErrorKind::Definition,
                format!("cannot define `{name}` here"),
                span,
            )),
        }
    }

    // ----- names ---------------------------------------------------------

    /// Converts a number to the evaluation mode (`f64` in plot mode).
    pub(crate) fn mode_num(&self, n: Number) -> Number {
        if self.opts.float64 {
            n.into_f64_kind()
        } else {
            n
        }
    }

    pub(crate) fn pi(&mut self) -> EvalResult<Number> {
        if self.opts.float64 {
            return Ok(Number::F64(std::f64::consts::PI));
        }
        if let Some(p) = &self.pi {
            return Ok(p.clone());
        }
        let p = Number::pi(self.opts.precision).at(Span::default())?;
        self.pi = Some(p.clone());
        Ok(p)
    }

    fn constant(&mut self, name: &str, span: Span) -> EvalResult<Option<Value>> {
        let prec = self.opts.precision;
        if !self.opts.float64 {
            use crate::cas::Sym;
            let s = match name {
                "π" => Some(Sym::pi()),
                "τ" => Some(Sym::int(2) * Sym::pi()),
                "e" => Some(Sym::e()),
                "φ" => Some((Sym::one() + Sym::int(5).sqrt()) / Sym::int(2)),
                _ => None,
            };
            if let Some(s) = s {
                return Ok(Some(Value::sym(s)));
            }
        }
        let n = match name {
            "π" => self.pi()?,
            "τ" => self.pi()? * Number::from(2),
            "e" => {
                if self.opts.float64 {
                    Number::F64(std::f64::consts::E)
                } else {
                    Number::e(prec).at(span)?
                }
            }
            "φ" => {
                let s = Number::from(5).sqrt(prec).at(span)?;
                let phi = (Number::one() + s).checked_div(&Number::from(2)).at(span)?;
                self.mode_num(phi)
            }
            "i" => Number::i(),
            "true" => return Ok(Some(Value::Bool(true))),
            "false" => return Ok(Some(Value::Bool(false))),
            "∞" if self.symbolic => {
                return Ok(Some(Value::Expr(super::value::SymValue::new(
                    crate::cas::limit::infinity(),
                ))));
            }
            "∞" => {
                return Err(EvalError::unsupported(
                    "`∞` can only be used as a bound of ∫, ∑, ∏ or in CAS functions (limit)",
                    span,
                ));
            }
            _ => return Ok(None),
        };
        Ok(Some(Value::Number(n)))
    }

    /// Resolves a (canonical) name without splitting: locals, constants,
    /// builtins, globals, resolver.
    pub(crate) fn lookup_known(&mut self, name: &str, span: Span) -> EvalResult<Option<Value>> {
        if let Some(v) = self.lookup_local(name) {
            return Ok(Some(v.clone()));
        }
        if let Some(v) = self.constant(name, span)? {
            return Ok(Some(v));
        }
        if let Some(v) = builtins::function_value(name) {
            return Ok(Some(v));
        }
        Ok(self.env().lookup(name))
    }

    /// Does `name` resolve without splitting?
    pub(crate) fn is_known(&self, name: &str) -> bool {
        self.lookup_local(name).is_some()
            || CONSTANTS.contains(&name)
            || builtins::lookup(name).is_some()
            || self.env().has(name)
    }

    /// Splits an unknown multi-letter name into known pieces (`xy` → `x·y`,
    /// `ab2` → `a·b2`), preferring the longest known prefix at each position.
    /// `None` unless every piece resolves.
    pub(crate) fn split_name(&self, name: &str) -> Option<Vec<String>> {
        split_with(name, &|p| self.is_known(canonical_name(p)))
            .map(|v| v.iter().map(|p| canonical_name(p).to_string()).collect())
    }

    /// Evaluates an identifier.
    fn eval_ident(&mut self, name: &str, span: Span) -> EvalResult<Value> {
        let name = canonical_name(name);
        if let Some(v) = self.lookup_known(name, span)? {
            return Ok(v);
        }
        if let Some(pieces) = self.split_name(name) {
            return self.split_product(&pieces, span);
        }
        if self.symbolic {
            return self.symbol_value(name, span);
        }
        Err(unknown_name(name, span))
    }

    /// An unknown name in symbolic mode: a symbol, or a product of
    /// symbols and known values for a splittable name (`ab` = `a·b`).
    fn symbol_value(&mut self, name: &str, span: Span) -> EvalResult<Value> {
        use crate::cas::Sym;
        let single = |p: &str| {
            canonical_name(p)
                .chars()
                .filter(|c| c.is_alphabetic())
                .count()
                == 1
        };
        if single(name) || name.chars().count() > MAX_SPLIT_LEN {
            return Ok(Value::Expr(super::value::SymValue::new(Sym::var(name))));
        }
        let Some(pieces) = split_with(name, &|p| self.is_known(canonical_name(p)) || single(p))
        else {
            return Ok(Value::Expr(super::value::SymValue::new(Sym::var(name))));
        };
        let mut acc: Option<Value> = None;
        for p in pieces {
            let p = canonical_name(&p).to_string();
            let v = match self.lookup_known(&p, span)? {
                Some(v) => v,
                None => Value::Expr(super::value::SymValue::new(Sym::var(&p))),
            };
            acc = Some(match acc {
                None => v,
                Some(a) => self.arith(Arith::Mul, a, v, span)?,
            });
        }
        Ok(acc.expect("at least one piece"))
    }

    /// Product of split pieces; a function piece applies to the product of
    /// the pieces after it (`sinx` = `sin(x)` when `x` is known).
    fn split_product(&mut self, pieces: &[String], span: Span) -> EvalResult<Value> {
        let mut values = Vec::with_capacity(pieces.len());
        for p in pieces {
            values.push(self.lookup_known(p, span)?.expect("split pieces are known"));
        }
        let mut acc: Option<Value> = None;
        while let Some(v) = values.pop() {
            acc = Some(match (v, acc) {
                (v, None) => v,
                (Value::Function(f), Some(a)) => self.call_function(&f, vec![a], span)?,
                (v, Some(a)) => self.arith(Arith::Mul, v, a, span)?,
            });
        }
        Ok(acc.expect("at least two pieces"))
    }

    // ----- expressions ---------------------------------------------------

    /// Evaluates an expression.
    pub fn eval(&mut self, e: &Expr) -> EvalResult<Value> {
        self.tick(e.span)?;
        self.enter(e.span)?;
        let r = self.eval_kind(e);
        self.depth -= 1;
        r
    }

    /// Thin dispatcher: every arm is a separate non-inlined function so the
    /// frame of this recursive function stays small (deep recursion must
    /// fit in a 2 MiB stack even in debug builds).
    fn eval_kind(&mut self, e: &Expr) -> EvalResult<Value> {
        let span = e.span;
        match &*e.kind {
            ExprKind::Number { text, imaginary } => self.eval_literal(text, *imaginary, span),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Ident(name) => self.eval_ident(name, span),
            ExprKind::Prime { .. } => Err(EvalError::ty(
                "a derivative `f'` must be applied to an argument: write `f'(x)`",
                span,
            )),
            ExprKind::Unary { op, expr } => self.eval_unary(*op, expr, span),
            ExprKind::Binary { op, lhs, rhs } => self.eval_binary(*op, lhs, rhs, span),
            ExprKind::Add(terms) => self.eval_add(terms, span),
            ExprKind::Mul(factors) => self.eval_mul(factors),
            ExprKind::And(items) => self.eval_logic(items, true, span),
            ExprKind::Or(items) => self.eval_logic(items, false, span),
            ExprKind::Postfix { op, expr } => self.eval_postfix(*op, expr, span),
            ExprKind::Compare { first, rest } => self.eval_compare(first, rest, span),
            ExprKind::Call { callee, args } => self.eval_call(callee, args, span),
            ExprKind::Member { expr, name } => self.eval_member(expr, name, span),
            ExprKind::Index { expr, indices } => self.eval_index(expr, indices),
            ExprKind::List(items) => {
                let v = self.eval_items(items)?;
                check_nesting(&v, span)?;
                Ok(make_list(v))
            }
            ExprKind::Range { start, second, end } => {
                self.eval_range(start, second.as_ref(), end, span)
            }
            ExprKind::Comprehension { body, bindings } => self.eval_comprehension(body, bindings),
            ExprKind::Tuple(items) => {
                let v = self.eval_items(items)?;
                check_nesting(&v, span)?;
                Ok(Value::Tuple(v))
            }
            ExprKind::Piecewise {
                branches,
                otherwise,
            } => self.eval_piecewise(branches, otherwise.as_ref(), span),
            ExprKind::Restrict { expr, conds } => self.eval_restricted(expr, conds),
            ExprKind::Derivative { var, order, expr } => {
                self.eval_derivative(var, *order, expr, span)
            }
            ExprKind::Integral { bounds, body, var } => {
                self.eval_integral(bounds.as_ref(), body, var, span)
            }
            ExprKind::BigOp {
                op,
                var,
                from,
                to,
                body,
            } => self.eval_bigop(*op, var, from, to, body, span),
        }
    }

    #[inline(never)]
    fn eval_add(&mut self, terms: &[Term], span: Span) -> EvalResult<Value> {
        let mut acc = self.eval(&terms[0].expr)?;
        if terms[0].op == AddOp::Sub {
            acc = self.negate(acc, span)?;
        }
        for t in &terms[1..] {
            let v = self.eval(&t.expr)?;
            let op = match t.op {
                AddOp::Add => Arith::Add,
                AddOp::Sub => Arith::Sub,
            };
            acc = self.arith(op, acc, v, t.expr.span)?;
        }
        Ok(acc)
    }

    #[inline(never)]
    fn eval_mul(&mut self, factors: &[Factor]) -> EvalResult<Value> {
        let mut acc = self.eval(&factors[0].expr)?;
        for f in &factors[1..] {
            let v = self.eval(&f.expr)?;
            let op = match f.op {
                MulOp::Mul => Arith::Mul,
                MulOp::Div => Arith::Div,
            };
            acc = self.arith(op, acc, v, f.expr.span)?;
        }
        Ok(acc)
    }

    #[inline(never)]
    fn eval_postfix(&mut self, op: PostfixOp, expr: &Expr, span: Span) -> EvalResult<Value> {
        let v = self.eval(expr)?;
        match op {
            PostfixOp::Factorial => {
                let prec = self.opts.precision;
                self.map_num(&v, span, &|n| n.factorial(prec))
            }
            PostfixOp::Percent => self.arith(Arith::Div, v, Value::num(Number::from(100)), span),
        }
    }

    #[inline(never)]
    fn eval_compare(
        &mut self,
        first: &Expr,
        rest: &[(crate::syntax::CmpOp, Expr)],
        span: Span,
    ) -> EvalResult<Value> {
        let mut lhs = self.eval(first)?;
        let mut acc: Option<Value> = None;
        for (op, e) in rest {
            let rhs = self.eval(e)?;
            let r = self.compare(&lhs, *op, &rhs, span)?;
            acc = Some(match acc {
                None => r,
                Some(a) => self.logic2(a, r, true, span)?,
            });
            lhs = rhs;
        }
        Ok(acc.expect("comparison has an operator"))
    }

    #[inline(never)]
    fn eval_member(&mut self, expr: &Expr, name: &str, span: Span) -> EvalResult<Value> {
        let v = self.eval(expr)?;
        self.member(v, name, span)
    }

    #[inline(never)]
    fn eval_index(&mut self, expr: &Expr, indices: &[Expr]) -> EvalResult<Value> {
        let mut v = self.eval(expr)?;
        for idx in indices {
            let i = self.eval(idx)?;
            v = self.index(v, i, idx.span)?;
        }
        Ok(v)
    }

    #[inline(never)]
    fn eval_items(&mut self, items: &[Expr]) -> EvalResult<Vec<Value>> {
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            out.push(self.eval(it)?);
        }
        Ok(out)
    }

    #[inline(never)]
    fn eval_range(
        &mut self,
        start: &Expr,
        second: Option<&Expr>,
        end: &Expr,
        span: Span,
    ) -> EvalResult<Value> {
        let a = self.eval(start)?;
        let s = match second {
            Some(s) => Some(self.eval(s)?),
            None => None,
        };
        let b = self.eval(end)?;
        let (a, b) = (
            self.expect_num(a, start.span)?,
            self.expect_num(b, end.span)?,
        );
        let step = match s {
            Some(s) => {
                let s = self.expect_num(s, span)?;
                Some(&s - &a)
            }
            None => None,
        };
        self.range(&a, &b, step, span)
    }

    #[inline(never)]
    fn eval_comprehension(
        &mut self,
        body: &Expr,
        bindings: &[crate::syntax::Binding],
    ) -> EvalResult<Value> {
        let mut out = Vec::new();
        self.push_scope(Vec::new());
        let r = self.comprehension(body, bindings, 0, &mut out);
        self.pop_scope();
        r?;
        check_nesting(&out, body.span)?;
        Ok(make_list(out))
    }

    fn eval_literal(&mut self, text: &str, imaginary: bool, span: Span) -> EvalResult<Value> {
        let n = parse_literal(text).at(span)?;
        let n = if imaginary { n.imaginary() } else { n };
        Ok(Value::Number(self.mode_num(n)))
    }

    fn eval_unary(&mut self, op: UnaryOp, expr: &Expr, span: Span) -> EvalResult<Value> {
        let v = self.eval(expr)?;
        match op {
            UnaryOp::Neg => self.negate(v, span),
            UnaryOp::Pos => match v {
                Value::Bool(_) | Value::Str(_) | Value::Function(_) => Err(EvalError::ty(
                    format!("cannot apply `+` to a {}", v.type_name()),
                    span,
                )),
                v => Ok(v),
            },
            UnaryOp::Not => self.logic_not(v, span),
        }
    }

    fn eval_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> EvalResult<Value> {
        if op == BinOp::To {
            return Err(EvalError::unsupported(
                "units are not supported yet (planned milestone)",
                span,
            ));
        }
        let a = self.eval(lhs)?;
        let b = self.eval(rhs)?;
        match op {
            BinOp::Pow => self.arith(Arith::Pow, a, b, span),
            BinOp::Mod => self.arith(Arith::Mod, a, b, span),
            BinOp::To => unreachable!(),
        }
    }

    fn eval_logic(&mut self, items: &[Expr], is_and: bool, span: Span) -> EvalResult<Value> {
        let mut acc = self.eval(&items[0])?;
        for it in &items[1..] {
            // Short-circuit on scalars.
            if let Value::Bool(b) = acc
                && b != is_and
            {
                return Ok(acc);
            }
            let v = self.eval(it)?;
            acc = self.logic2(acc, v, is_and, span)?;
        }
        if let Value::Number(_) | Value::Str(_) | Value::Tuple(_) | Value::Function(_) = acc {
            return Err(EvalError::ty("`and`/`or` need true/false operands", span));
        }
        Ok(acc)
    }

    /// Condition truth: `Undefined` counts as false.
    pub(crate) fn truth(&self, v: &Value, span: Span) -> EvalResult<bool> {
        match v {
            Value::Bool(b) => Ok(*b),
            Value::Undefined => Ok(false),
            other => Err(EvalError::ty(
                format!(
                    "condition must be true or false, got a {}",
                    other.type_name()
                ),
                span,
            )),
        }
    }

    fn eval_piecewise(
        &mut self,
        branches: &[Branch],
        otherwise: Option<&Expr>,
        span: Span,
    ) -> EvalResult<Value> {
        let mut conds = Vec::with_capacity(branches.len());
        for (k, b) in branches.iter().enumerate() {
            let c = self.eval(&b.cond)?;
            if let Value::List(_) = c {
                conds.push(c);
                return self.piecewise_lists(branches, otherwise, conds, k + 1, span);
            }
            if self.truth(&c, b.cond.span)? {
                return match &b.value {
                    Some(v) => self.eval(v),
                    None => Ok(Value::num(self.mode_num(Number::one()))),
                };
            }
            conds.push(c);
        }
        match otherwise {
            Some(o) => self.eval(o),
            None => Ok(Value::Undefined),
        }
    }

    /// Piecewise with list-valued conditions: selects element-wise. The
    /// branch index of every element is computed first; later conditions
    /// are only evaluated while some element is still unresolved, and a
    /// branch value only if some element selects it.
    #[inline(never)]
    fn piecewise_lists(
        &mut self,
        branches: &[Branch],
        otherwise: Option<&Expr>,
        conds: Vec<Value>,
        from: usize,
        span: Span,
    ) -> EvalResult<Value> {
        let len = match conds.last() {
            Some(Value::List(l)) => l.len(),
            _ => unreachable!("called with a list condition last"),
        };
        let pick = |v: &Value, i: usize| -> EvalResult<Value> {
            match v {
                Value::List(l) if l.len() == len => Ok(l[i].clone()),
                Value::List(_) => Err(EvalError::other("list lengths differ in piecewise", span)),
                other => Ok(other.clone()),
            }
        };
        // Earlier scalar conditions were all false; `None` = unresolved.
        let mut choice: Vec<Option<usize>> = vec![None; len];
        let mut k = from - 1;
        let mut cond = conds.into_iter().last();
        loop {
            let c = cond.take().expect("condition");
            self.charge(len as u64, span)?;
            for (i, ch) in choice.iter_mut().enumerate() {
                if ch.is_none() && self.truth(&pick(&c, i)?, span)? {
                    *ch = Some(k);
                }
            }
            k += 1;
            if k >= branches.len() || choice.iter().all(Option::is_some) {
                break;
            }
            cond = Some(self.eval(&branches[k].cond)?);
        }
        let mut values: Vec<Option<Value>> = vec![None; branches.len()];
        for b in choice.iter().flatten() {
            if values[*b].is_none() {
                values[*b] = Some(match &branches[*b].value {
                    Some(v) => self.eval(v)?,
                    None => Value::num(self.mode_num(Number::one())),
                });
            }
        }
        let other = match otherwise {
            Some(o) if choice.iter().any(Option::is_none) => self.eval(o)?,
            _ => Value::Undefined,
        };
        let mut out = Vec::with_capacity(len);
        for (i, ch) in choice.iter().enumerate() {
            out.push(match ch {
                Some(b) => pick(values[*b].as_ref().expect("evaluated"), i)?,
                None => pick(&other, i)?,
            });
        }
        Ok(Value::List(out))
    }

    /// `expr {c1, c2}`: the value where every condition holds, else
    /// `Undefined` (element-wise for list conditions).
    pub(crate) fn eval_restricted(&mut self, expr: &Expr, conds: &[Expr]) -> EvalResult<Value> {
        let mut mask: Option<Vec<bool>> = None;
        for c in conds {
            let v = self.eval(c)?;
            match v {
                Value::List(items) => {
                    let mut bits = Vec::with_capacity(items.len());
                    for it in &items {
                        bits.push(self.truth(it, c.span)?);
                    }
                    mask = Some(match mask {
                        None => bits,
                        Some(m) if m.len() == bits.len() => {
                            m.iter().zip(&bits).map(|(a, b)| *a && *b).collect()
                        }
                        Some(_) => {
                            return Err(EvalError::other(
                                "restriction lists differ in length",
                                c.span,
                            ));
                        }
                    });
                }
                other => {
                    if !self.truth(&other, c.span)? {
                        return Ok(Value::Undefined);
                    }
                }
            }
        }
        let value = self.eval(expr)?;
        match mask {
            None => Ok(value),
            Some(m) => match value {
                Value::List(items) if items.len() == m.len() => Ok(Value::List(
                    items
                        .into_iter()
                        .zip(m)
                        .map(|(v, keep)| if keep { v } else { Value::Undefined })
                        .collect(),
                )),
                Value::List(items) => Err(EvalError::other(
                    format!(
                        "restriction has {} conditions for a list of {} elements",
                        m.len(),
                        items.len()
                    ),
                    expr.span,
                )),
                other => Ok(Value::List(
                    m.into_iter()
                        .map(|keep| {
                            if keep {
                                other.clone()
                            } else {
                                Value::Undefined
                            }
                        })
                        .collect(),
                )),
            },
        }
    }

    fn comprehension(
        &mut self,
        body: &Expr,
        bindings: &[crate::syntax::Binding],
        k: usize,
        out: &mut Vec<Value>,
    ) -> EvalResult<()> {
        let Some(b) = bindings.get(k) else {
            if out.len() >= MAX_LIST_LEN {
                return Err(EvalError::new(
                    ErrorKind::Other,
                    "comprehension produces too many elements",
                    body.span,
                ));
            }
            self.alloc(1, body.span)?;
            out.push(self.eval(body)?);
            return Ok(());
        };
        let iter = self.eval(&b.iter)?;
        let items = self.iterable(iter, b.iter.span)?;
        for item in items {
            self.bind_local(&b.name, item);
            self.comprehension(body, bindings, k + 1, out)?;
        }
        Ok(())
    }

    /// Elements to iterate over: list items, matrix rows.
    pub(crate) fn iterable(&self, v: Value, span: Span) -> EvalResult<Vec<Value>> {
        match v {
            Value::List(items) => Ok(items),
            Value::Matrix(m) => Ok(m.to_rows()),
            other => Err(EvalError::ty(
                format!(
                    "expected a list to iterate over, got a {}",
                    other.type_name()
                ),
                span,
            )),
        }
    }

    pub(crate) fn expect_num(&self, v: Value, span: Span) -> EvalResult<Number> {
        match v {
            Value::Number(n) => Ok(n),
            Value::Expr(s) => self.expr_number(&s, span),
            other => Err(EvalError::ty(
                format!("expected a number, got a {}", other.type_name()),
                span,
            )),
        }
    }

    /// Exact (or float) arithmetic range `a, a+step, ... ≤ b`.
    pub(crate) fn range(
        &mut self,
        a: &Number,
        b: &Number,
        step: Option<Number>,
        span: Span,
    ) -> EvalResult<Value> {
        let step = match step {
            Some(s) => s,
            None => {
                let up = a.cmp_real(b).at(span)? != std::cmp::Ordering::Greater;
                self.mode_num(Number::from(if up { 1 } else { -1 }))
            }
        };
        if step.is_zero() {
            return Err(EvalError::other("range step is zero", span));
        }
        let span_len = b - a;
        let count = span_len.checked_div(&step).at(span)?;
        if count.is_negative() {
            return Err(EvalError::other(
                "range step goes away from the end value",
                span,
            ));
        }
        // Tolerate float rounding: 0.1 * 10 may land just below 1.
        let count_f = count.to_f64();
        let n = if count.is_exact() {
            count.floor().at(span)?.to_f64()
        } else {
            (count_f + 1e-9).floor()
        };
        if !n.is_finite() || n + 1.0 > MAX_LIST_LEN as f64 {
            return Err(EvalError::other("range has too many elements", span));
        }
        let n = n as usize;
        self.alloc(n as u64 + 1, span)?;
        let mut out = Vec::with_capacity(n + 1);
        for k in 0..=n {
            out.push(Value::Number(a + &(&step * &Number::from(k as u64))));
        }
        Ok(Value::List(out))
    }

    // ----- calls ---------------------------------------------------------

    fn eval_args(&mut self, args: &[Expr]) -> EvalResult<Vec<Value>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(self.eval(a)?);
        }
        Ok(out)
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> EvalResult<Value> {
        if let ExprKind::Ident(name) = &*callee.kind {
            let name = canonical_name(name);
            if let Some(v) = self.lookup_known(name, callee.span)? {
                return self.apply_or_multiply(v, args, span);
            }
        }
        self.eval_call_slow(callee, args, span)
    }

    #[inline(never)]
    fn eval_call_slow(&mut self, callee: &Expr, args: &[Expr], span: Span) -> EvalResult<Value> {
        match &*callee.kind {
            ExprKind::Ident(name) => {
                let name = canonical_name(name);
                if let Some(base) = name.strip_prefix("log_") {
                    return self.call_log_base(base, args, callee.span, span);
                }
                if let Some(pieces) = self.split_name(name) {
                    let (last, init) = pieces.split_last().expect("pieces");
                    let mut acc: Option<Value> = None;
                    for p in init {
                        let v = self.lookup_known(p, callee.span)?.expect("known");
                        acc = Some(match acc {
                            None => v,
                            Some(a) => self.arith(Arith::Mul, a, v, span)?,
                        });
                    }
                    let lastv = self.lookup_known(last, callee.span)?.expect("known");
                    let applied = self.apply_or_multiply(lastv, args, span)?;
                    return match acc {
                        None => Ok(applied),
                        Some(a) => self.arith(Arith::Mul, a, applied, span),
                    };
                }
                if self.symbolic {
                    // An unknown "function" multiplies: f(x) = f·x.
                    let v = self.symbol_value(name, callee.span)?;
                    return self.apply_or_multiply(v, args, span);
                }
                Err(unknown_name(name, callee.span))
            }
            ExprKind::Prime { expr, order } => {
                let f = self.eval(expr)?;
                let Value::Function(f) = f else {
                    return Err(EvalError::ty(
                        format!("`'` needs a function, got a {}", f.type_name()),
                        expr.span,
                    ));
                };
                let argv = self.eval_args(args)?;
                self.call_prime(&f, *order, argv, span)
            }
            ExprKind::Member { expr, name } => {
                let recv = self.eval(expr)?;
                if let Some(v) = self.try_member(&recv, name) {
                    return self.apply_or_multiply(v, args, span);
                }
                let fname = canonical_name(name);
                match self.lookup_known(fname, callee.span)? {
                    Some(Value::Function(f)) => {
                        let mut argv = vec![recv];
                        argv.extend(self.eval_args(args)?);
                        self.call_function(&f, argv, span)
                    }
                    _ => Err(EvalError::ty(
                        format!("a {} has no member or method `{name}`", recv.type_name()),
                        callee.span,
                    )),
                }
            }
            _ => {
                let v = self.eval(callee)?;
                self.apply_or_multiply(v, args, span)
            }
        }
    }

    /// `v(args)`: a call when `v` is a function, otherwise multiplication
    /// (`a(b + 1)` = `a·(b + 1)`, `a(1, 2)` = `a·(1, 2)`).
    fn apply_or_multiply(&mut self, v: Value, args: &[Expr], span: Span) -> EvalResult<Value> {
        if let Value::Function(f) = &v
            && let Function::Builtin(b) = &**f
            && b.symbolic
        {
            if b.name == "steps" {
                return self.eval_steps(args, span);
            }
            let argv = self.eval_symbolic_args(b, args)?;
            return self.call_function(f, argv, span);
        }
        let argv = self.eval_args(args)?;
        match v {
            Value::Function(f) => self.call_function(&f, argv, span),
            other => {
                let rhs = match argv.len() {
                    0 => {
                        return Err(EvalError::ty(
                            format!("a {} cannot be called", other.type_name()),
                            span,
                        ));
                    }
                    1 => argv.into_iter().next().expect("one"),
                    _ => {
                        check_nesting(&argv, span)?;
                        Value::Tuple(argv)
                    }
                };
                self.arith(Arith::Mul, other, rhs, span)
            }
        }
    }

    fn call_log_base(
        &mut self,
        base: &str,
        args: &[Expr],
        name_span: Span,
        span: Span,
    ) -> EvalResult<Value> {
        let b = match parse_literal(base) {
            Ok(n) => Value::Number(self.mode_num(n)),
            Err(_) => {
                let base = canonical_name(base);
                match self.lookup_known(base, name_span)? {
                    Some(v) => v,
                    None => return Err(unknown_name(base, name_span)),
                }
            }
        };
        let mut argv = self.eval_args(args)?;
        if argv.len() != 1 {
            return Err(EvalError::new(
                ErrorKind::Arity,
                "`log_b` takes one argument",
                span,
            ));
        }
        argv.push(b);
        let f = builtins::function_value("log").expect("log builtin");
        let Value::Function(f) = f else {
            unreachable!()
        };
        self.call_function(&f, argv, span)
    }

    /// Calls a function value with evaluated arguments.
    pub fn call_function(
        &mut self,
        f: &Arc<Function>,
        args: Vec<Value>,
        span: Span,
    ) -> EvalResult<Value> {
        match &**f {
            Function::Builtin(b) => {
                if args.len() < b.min || args.len() > b.max {
                    return Err(arity_error(b.name, b.min, b.max, args.len(), span));
                }
                if let Some(r) = self.try_symbolic_call(b, &args, span) {
                    return r;
                }
                (b.f)(self, args, span)
            }
            Function::User(u) => {
                if args.len() != u.params.len() {
                    let n = u.params.len();
                    return Err(arity_error(f.name(), n, n, args.len(), span));
                }
                self.enter(span)?;
                let mut frame = Vec::with_capacity(u.params.len() + 1);
                if let Some(name) = &u.name {
                    frame.push((name.clone(), Value::Function(f.clone())));
                }
                for (p, a) in u.params.iter().zip(args) {
                    frame.push((p.clone(), a));
                }
                let saved_base = self.base;
                let saved_captured = std::mem::replace(&mut self.captured, u.captured.clone());
                self.base = self.scopes.len();
                self.scopes.push(frame);
                let r = match &u.body {
                    FnBody::Expr(e) => self.eval(e),
                    FnBody::Block(b) => self.exec_stmts(&b.stmts).map(Flow::into_value),
                };
                self.scopes.truncate(self.base);
                self.base = saved_base;
                self.captured = saved_captured;
                self.depth -= 1;
                r
            }
        }
    }

    /// `f'(x)`, `f''(x)`: numeric derivative of a one-argument function.
    fn call_prime(
        &mut self,
        f: &Arc<Function>,
        order: u32,
        args: Vec<Value>,
        span: Span,
    ) -> EvalResult<Value> {
        if args.len() != 1 {
            return Err(EvalError::new(
                ErrorKind::Arity,
                "`f'` takes exactly one argument",
                span,
            ));
        }
        let arg = args.into_iter().next().expect("one");
        if let Some(v) = self.symbolic_prime(f, order, &arg, span)? {
            return Ok(v);
        }
        self.map_value(arg, span, &mut |ev, x| {
            let x0 = ev.real_f64(&x, span)?;
            let d = ev.numeric_derivative(order, x0, span, &mut |ev, t| {
                let v = ev.call_function(f, vec![Value::num(Number::F64(t))], span)?;
                ev.real_f64(&v, span)
            })?;
            Ok(Value::Number(Number::f64(d).at(span)?))
        })
    }

    /// Applies `f` to a number or element-wise to a list.
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn map_value(
        &mut self,
        v: Value,
        span: Span,
        f: &mut dyn FnMut(&mut Self, Value) -> EvalResult<Value>,
    ) -> EvalResult<Value> {
        match v {
            Value::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.map_value(it, span, f)?);
                }
                Ok(Value::List(out))
            }
            Value::Undefined => Ok(Value::Undefined),
            other => f(self, other),
        }
    }

    /// Real `f64` of a value (error for non-numbers / non-real numbers).
    pub(crate) fn real_f64(&self, v: &Value, span: Span) -> EvalResult<f64> {
        match v {
            Value::Number(n) if n.is_real() => Ok(n.to_f64()),
            Value::Number(_) => Err(EvalError::new(
                ErrorKind::NotReal,
                "expected a real number",
                span,
            )),
            Value::Expr(s) => {
                let n = self.expr_number(s, span)?;
                self.real_f64(&Value::Number(n), span)
            }
            Value::Undefined => Ok(f64::NAN),
            other => Err(EvalError::ty(
                format!("expected a number, got a {}", other.type_name()),
                span,
            )),
        }
    }

    // ----- statements ----------------------------------------------------

    pub(crate) fn exec_block(&mut self, block: &Block) -> EvalResult<Flow> {
        self.push_scope(Vec::new());
        let r = self.exec_stmts(&block.stmts);
        self.pop_scope();
        r
    }

    pub(crate) fn exec_stmts(&mut self, stmts: &[Stmt]) -> EvalResult<Flow> {
        let mut last = Value::Undefined;
        for s in stmts {
            match self.exec_stmt(s)? {
                Flow::Value(v) => last = v,
                ret @ Flow::Return(_) => return Ok(ret),
            }
        }
        Ok(Flow::Value(last))
    }

    /// Executes one statement in script context: `name = value` assigns a
    /// visible `let` variable, otherwise defines a global.
    pub(crate) fn exec_stmt(&mut self, s: &Stmt) -> EvalResult<Flow> {
        self.tick(s.span)?;
        let span = s.span;
        match &s.kind {
            StmtKind::Expr { expr, restrictions } => self.exec_expr_stmt(expr, restrictions),
            StmtKind::Equation {
                lhs,
                rhs,
                restrictions,
            } => self
                .assign_equation(lhs, rhs, restrictions, span)
                .map(Flow::Value),
            StmtKind::Definition {
                lhs,
                rhs,
                restrictions,
                with_eq,
            } => self
                .exec_definition(lhs, rhs, restrictions, *with_eq)
                .map(|(_, v)| Flow::Value(v)),
            StmtKind::Regression { .. } => Err(EvalError::unsupported(
                "regression (`~`) is not implemented yet",
                span,
            )),
            StmtKind::Action { actions } => self.exec_action(actions).map(Flow::Value),
            StmtKind::Let { name, value } => self.exec_let(name, value, span),
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => self.exec_if(cond, then, otherwise.as_ref()),
            StmtKind::For { var, iter, body } => self.exec_for(var, iter, body, span),
            StmtKind::While { cond, body } => self.exec_while(cond, body, span),
            StmtKind::Fn { name, params, body } => self.exec_fn(name, params, body, span),
            StmtKind::Return(e) => self.exec_return(e.as_ref()),
        }
    }

    #[inline(never)]
    fn exec_expr_stmt(&mut self, expr: &Expr, restrictions: &[Expr]) -> EvalResult<Flow> {
        let v = if restrictions.is_empty() {
            self.eval(expr)?
        } else {
            self.eval_restricted(expr, restrictions)?
        };
        Ok(Flow::Value(v))
    }

    #[inline(never)]
    fn exec_let(&mut self, name: &str, value: &Expr, span: Span) -> EvalResult<Flow> {
        let v = self.eval(value)?;
        if self.has_local_scope() {
            self.bind_local(name, v.clone());
        } else {
            self.define_global(name, v.clone(), span)?;
        }
        Ok(Flow::Value(v))
    }

    #[inline(never)]
    fn exec_if(
        &mut self,
        cond: &Expr,
        then: &Block,
        otherwise: Option<&ElseBranch>,
    ) -> EvalResult<Flow> {
        let c = self.eval(cond)?;
        if self.truth(&c, cond.span)? {
            self.exec_block(then)
        } else {
            match otherwise {
                Some(ElseBranch::Block(b)) => self.exec_block(b),
                Some(ElseBranch::If(s)) => self.exec_stmt(s),
                None => Ok(Flow::Value(Value::Undefined)),
            }
        }
    }

    #[inline(never)]
    fn exec_for(&mut self, var: &str, iter: &Expr, body: &Block, span: Span) -> EvalResult<Flow> {
        let it = self.eval(iter)?;
        let items = self.iterable(it, iter.span)?;
        for item in items {
            self.tick(span)?;
            self.push_scope(vec![(canonical_name(var).to_string(), item)]);
            let r = self.exec_stmts(&body.stmts);
            self.pop_scope();
            if let ret @ Flow::Return(_) = r? {
                return Ok(ret);
            }
        }
        Ok(Flow::Value(Value::Undefined))
    }

    #[inline(never)]
    fn exec_while(&mut self, cond: &Expr, body: &Block, span: Span) -> EvalResult<Flow> {
        loop {
            self.tick(span)?;
            let c = self.eval(cond)?;
            if !self.truth(&c, cond.span)? {
                break;
            }
            if let ret @ Flow::Return(_) = self.exec_block(body)? {
                return Ok(ret);
            }
        }
        Ok(Flow::Value(Value::Undefined))
    }

    #[inline(never)]
    fn exec_fn(
        &mut self,
        name: &str,
        params: &[String],
        body: &Block,
        span: Span,
    ) -> EvalResult<Flow> {
        let f = self.make_fn(name, params, FnBody::Block(body.clone()));
        if self.has_local_scope() {
            self.bind_local(name, f.clone());
        } else {
            self.define_global(name, f.clone(), span)?;
        }
        Ok(Flow::Value(f))
    }

    #[inline(never)]
    fn exec_return(&mut self, e: Option<&Expr>) -> EvalResult<Flow> {
        let v = match e {
            Some(e) => self.eval(e)?,
            None => Value::Undefined,
        };
        Ok(Flow::Return(v))
    }

    pub(crate) fn make_fn(&self, name: &str, params: &[String], body: FnBody) -> Value {
        Value::Function(Arc::new(Function::User(UserFn {
            name: Some(canonical_name(name).to_string()),
            params: params
                .iter()
                .map(|p| canonical_name(p).to_string())
                .collect(),
            body,
            captured: Arc::new(self.visible_locals()),
        })))
    }

    /// `name := value` / `f(x, y) := body`: always defines a global (a
    /// visible `let` of the same name keeps shadowing it locally). Only a
    /// definition written with `=` under `DefinitionOp::Eq` (`with_eq`)
    /// reassigns a visible `let` variable instead, so `=`-style scripts keep
    /// working.
    #[inline(never)]
    pub(crate) fn exec_definition(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        restrictions: &[Expr],
        with_eq: bool,
    ) -> EvalResult<(String, Value)> {
        let body = with_restrictions(rhs, restrictions);
        match &*lhs.kind {
            ExprKind::Ident(name) => {
                let name = canonical_name(name);
                let v = self.eval(&body)?;
                if !(with_eq && self.set_local(name, v.clone())) {
                    self.define_global(name, v.clone(), lhs.span)?;
                }
                Ok((name.to_string(), v))
            }
            ExprKind::Call { callee, args } => {
                let (name, params) = function_signature(callee, args).ok_or_else(|| {
                    EvalError::new(
                        ErrorKind::Definition,
                        "the left side of a definition must be a name or `f(x, ...)`",
                        lhs.span,
                    )
                })?;
                let f = self.make_fn(&name, &params, FnBody::Expr(body));
                self.define_global(&name, f.clone(), lhs.span)?;
                Ok((name, f))
            }
            _ => Err(EvalError::new(
                ErrorKind::Definition,
                "the left side of a definition must be a name or `f(x, ...)`",
                lhs.span,
            )),
        }
    }

    /// `x = value` in a script: reassigns a visible `let` variable. Any
    /// other `=` is an equation, which a script cannot execute.
    #[inline(never)]
    fn assign_equation(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        restrictions: &[Expr],
        span: Span,
    ) -> EvalResult<Value> {
        if let ExprKind::Ident(name) = &*lhs.kind {
            let name = canonical_name(name);
            if self.lookup_local(name).is_some() {
                let v = self.eval(&with_restrictions(rhs, restrictions))?;
                self.set_local(name, v.clone());
                return Ok(v);
            }
            return Err(EvalError::new(
                ErrorKind::Definition,
                format!(
                    "`{name} = ...` is an equation: use `{name} := ...` to define a global, or `let {name} = ...` for a local"
                ),
                span,
            ));
        }
        Err(EvalError::new(
            ErrorKind::Definition,
            "this is an equation; only `let` variables can be assigned with `=` (define with `:=`)",
            span,
        ))
    }

    /// `a -> a + 1, b -> 0`: all values are computed first, then assigned.
    fn exec_action(&mut self, actions: &[(Expr, Expr)]) -> EvalResult<Value> {
        let mut updates = Vec::with_capacity(actions.len());
        for (target, value) in actions {
            let ExprKind::Ident(name) = &*target.kind else {
                return Err(EvalError::new(
                    ErrorKind::Definition,
                    "an action target must be a name",
                    target.span,
                ));
            };
            updates.push((
                canonical_name(name).to_string(),
                self.eval(value)?,
                target.span,
            ));
        }
        let mut last = Value::Undefined;
        for (name, v, span) in updates {
            if !self.set_local(&name, v.clone()) {
                self.define_global(&name, v.clone(), span)?;
            }
            last = v;
        }
        Ok(last)
    }
}

/// `f(x, y)` with distinct plain parameter names → `("f", ["x", "y"])`.
pub(crate) fn function_signature(callee: &Expr, args: &[Expr]) -> Option<(String, Vec<String>)> {
    let ExprKind::Ident(name) = &*callee.kind else {
        return None;
    };
    let mut params: Vec<String> = Vec::with_capacity(args.len());
    for a in args {
        let ExprKind::Ident(p) = &*a.kind else {
            return None;
        };
        let p = canonical_name(p).to_string();
        if params.contains(&p) {
            return None;
        }
        params.push(p);
    }
    Some((canonical_name(name).to_string(), params))
}

/// Wraps `e` in a restriction node when there are restrictions.
pub(crate) fn with_restrictions(e: &Expr, restrictions: &[Expr]) -> Expr {
    if restrictions.is_empty() {
        e.clone()
    } else {
        Expr::new(
            ExprKind::Restrict {
                expr: e.clone(),
                conds: restrictions.to_vec(),
            },
            e.span,
        )
    }
}

/// Maximum nesting of lists/points inside each other. Bounding it keeps
/// every recursive walk over values (clone, drop, display, broadcasting)
/// shallow, so `x = [x]` in a loop is an error instead of a stack overflow.
pub(crate) const MAX_NESTING: usize = 32;

/// Nesting depth of a value (0 for scalars), counted up to `limit`.
fn nesting(v: &Value, limit: usize) -> usize {
    let items = match v {
        Value::List(items) | Value::Tuple(items) => items,
        Value::Matrix(_) => return 2,
        _ => return 0,
    };
    if limit == 0 {
        return 1;
    }
    1 + items
        .iter()
        .map(|i| nesting(i, limit - 1))
        .max()
        .unwrap_or(0)
}

/// Fails if wrapping `items` in a list/point would exceed [`MAX_NESTING`].
pub(crate) fn check_nesting(items: &[Value], span: Span) -> EvalResult<()> {
    let deepest = items
        .iter()
        .filter(|v| matches!(v, Value::List(_) | Value::Tuple(_)))
        .map(|v| nesting(v, MAX_NESTING))
        .max()
        .unwrap_or(0);
    if deepest >= MAX_NESTING {
        return Err(EvalError::new(
            ErrorKind::DepthLimit,
            format!("lists nested more than {MAX_NESTING} levels deep"),
            span,
        ));
    }
    Ok(())
}

/// A list literal of equal-length numeric rows becomes a [`Matrix`].
pub(crate) fn make_list(items: Vec<Value>) -> Value {
    if matches!(items.first(), Some(Value::List(_)))
        && let Some(m) = Matrix::from_rows(&items)
    {
        return Value::Matrix(m);
    }
    Value::List(items)
}

pub(crate) fn unknown_name(name: &str, span: Span) -> EvalError {
    EvalError::new(
        ErrorKind::UnknownName(name.to_string()),
        format!("unknown name `{name}`"),
        span,
    )
}

fn arity_error(name: &str, min: usize, max: usize, got: usize, span: Span) -> EvalError {
    let expected = if min == max {
        format!("{min}")
    } else if max == usize::MAX {
        format!("at least {min}")
    } else {
        format!("{min} to {max}")
    };
    let plural = if expected == "1" { "" } else { "s" };
    EvalError::new(
        ErrorKind::Arity,
        format!("`{name}` takes {expected} argument{plural}, got {got}"),
        span,
    )
}

/// Splits `name` into pieces accepted by `known`: at each position the
/// longest known prefix wins, with backtracking; trailing digits and a
/// subscript stay attached to the preceding letter (`ab2` → `a·b2`,
/// `xy_1` → `x·y_1`). Returns `None` for single-piece names.
pub(crate) fn split_with(name: &str, known: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let (stem, sub) = match name.find('_') {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, ""),
    };
    // Atoms: a letter plus the digits after it.
    let mut atoms: Vec<String> = Vec::new();
    for c in stem.chars() {
        if c.is_ascii_digit() {
            atoms.last_mut()?.push(c);
        } else {
            atoms.push(c.to_string());
        }
    }
    if atoms.len() < 2 || atoms.len() > MAX_SPLIT_LEN {
        return None;
    }
    if let Some(last) = atoms.last_mut() {
        last.push_str(sub);
    }
    /// Backtracking split from atom `at`; `failed[i]` memoizes positions
    /// from which no split exists, so the search is polynomial.
    fn go(
        atoms: &[String],
        at: usize,
        known: &dyn Fn(&str) -> bool,
        failed: &mut [bool],
        out: &mut Vec<String>,
    ) -> bool {
        if at == atoms.len() {
            return true;
        }
        if failed[at] {
            return false;
        }
        for end in (at + 1..=atoms.len()).rev() {
            if at == 0 && end == atoms.len() {
                continue; // the whole name is not a split
            }
            let piece: String = atoms[at..end].concat();
            if known(&piece) {
                out.push(piece);
                if go(atoms, end, known, failed, out) {
                    return true;
                }
                out.pop();
            }
        }
        failed[at] = true;
        false
    }
    let mut out = Vec::new();
    let mut failed = vec![false; atoms.len()];
    go(&atoms, 0, known, &mut failed, &mut out).then_some(out)
}

/// Is `name` a constant or builtin (cannot be a free variable)?
pub(crate) fn is_builtin_name(name: &str) -> bool {
    is_reserved(name)
}
