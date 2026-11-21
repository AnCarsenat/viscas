//! Operators on values: arithmetic with broadcasting, points, matrices,
//! comparisons, logic, indexing and member access.

use std::cmp::Ordering;

use num_traits::ToPrimitive;

use super::error::{ErrorKind, EvalError, EvalResult, NumSpan};
use super::interp::Evaluator;
use super::value::{Matrix, Value};
use crate::number::{Number, NumberError};
use crate::syntax::{CmpOp, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,
}

impl Arith {
    fn verb(self) -> &'static str {
        match self {
            Arith::Add => "add",
            Arith::Sub => "subtract",
            Arith::Mul => "multiply",
            Arith::Div => "divide",
            Arith::Pow => "raise",
            Arith::Mod => "take `mod` of",
        }
    }
}

fn type_mismatch(op: Arith, a: &Value, b: &Value, span: Span) -> EvalError {
    EvalError::ty(
        format!(
            "cannot {} a {} and a {}",
            op.verb(),
            a.type_name(),
            b.type_name()
        ),
        span,
    )
}

fn not_real(span: Span) -> EvalError {
    EvalError::new(
        ErrorKind::NotReal,
        "the result is not a real number (complex mode is off)",
        span,
    )
}

impl Evaluator<'_> {
    /// Binary arithmetic with list broadcasting.
    pub(crate) fn arith(&mut self, op: Arith, a: Value, b: Value, span: Span) -> EvalResult<Value> {
        match (a, b) {
            (Value::Undefined, _) | (_, Value::Undefined) => Ok(Value::Undefined),
            (Value::Number(x), Value::Number(y)) => {
                self.num_arith(op, &x, &y, span).map(Value::Number)
            }
            (Value::Matrix(m), Value::List(v)) if op == Arith::Mul => self.mat_vec(&m, &v, span),
            (Value::List(xs), Value::List(ys)) => {
                if xs.len() != ys.len() {
                    return Err(EvalError::other(
                        format!("list lengths differ ({} and {})", xs.len(), ys.len()),
                        span,
                    ));
                }
                let mut out = Vec::with_capacity(xs.len());
                for (x, y) in xs.into_iter().zip(ys) {
                    out.push(self.arith(op, x, y, span)?);
                }
                Ok(Value::List(out))
            }
            (Value::List(xs), b) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.arith(op, x, b.clone(), span)?);
                }
                Ok(Value::List(out))
            }
            (a, Value::List(ys)) => {
                let mut out = Vec::with_capacity(ys.len());
                for y in ys {
                    out.push(self.arith(op, a.clone(), y, span)?);
                }
                Ok(Value::List(out))
            }
            (Value::Matrix(m), Value::Matrix(n)) => self.mat_mat(op, &m, &n, span),
            (Value::Matrix(m), Value::Number(y)) => match op {
                Arith::Mul | Arith::Div => self.mat_map(&m, |ev, x| ev.num_arith(op, x, &y, span)),
                Arith::Pow => self.mat_pow(&m, &y, span),
                _ => Err(type_mismatch(
                    op,
                    &Value::Matrix(m),
                    &Value::Number(y),
                    span,
                )),
            },
            (Value::Number(x), Value::Matrix(m)) if op == Arith::Mul => {
                self.mat_map(&m, |ev, y| ev.num_arith(op, &x, y, span))
            }
            (Value::Tuple(xs), Value::Tuple(ys)) if matches!(op, Arith::Add | Arith::Sub) => {
                if xs.len() != ys.len() {
                    return Err(EvalError::other(
                        format!(
                            "points have different dimensions ({} and {})",
                            xs.len(),
                            ys.len()
                        ),
                        span,
                    ));
                }
                let mut out = Vec::with_capacity(xs.len());
                for (x, y) in xs.into_iter().zip(ys) {
                    out.push(self.arith(op, x, y, span)?);
                }
                Ok(Value::Tuple(out))
            }
            (Value::Tuple(xs), y @ Value::Number(_)) if matches!(op, Arith::Mul | Arith::Div) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.arith(op, x, y.clone(), span)?);
                }
                Ok(Value::Tuple(out))
            }
            (x @ Value::Number(_), Value::Tuple(ys)) if op == Arith::Mul => {
                let mut out = Vec::with_capacity(ys.len());
                for y in ys {
                    out.push(self.arith(op, x.clone(), y, span)?);
                }
                Ok(Value::Tuple(out))
            }
            (Value::Str(mut a), Value::Str(b)) if op == Arith::Add => {
                a.push_str(&b);
                Ok(Value::Str(a))
            }
            (a, b) => Err(type_mismatch(op, &a, &b, span)),
        }
    }

    pub(crate) fn num_arith(
        &mut self,
        op: Arith,
        x: &Number,
        y: &Number,
        span: Span,
    ) -> EvalResult<Number> {
        match op {
            Arith::Add => Ok(x + y),
            Arith::Sub => Ok(x - y),
            Arith::Mul => Ok(x * y),
            Arith::Div => x.checked_div(y).at(span),
            Arith::Mod => x.modulo(y).at(span),
            Arith::Pow => self.pow_num(x, y, span),
        }
    }

    /// `a^b`, exact when possible. In real mode a negative base with a
    /// fractional exponent uses real odd roots (`(-8)^(1/3) = -2`) and is an
    /// error for even roots.
    pub(crate) fn pow_num(&mut self, a: &Number, b: &Number, span: Span) -> EvalResult<Number> {
        let prec = self.opts.precision;
        if !self.opts.complex && a.is_real() && b.is_real() && a.is_negative() && !b.is_integral() {
            return match odd_root_exponent(b) {
                Some((p, q)) => {
                    let r = a.nth_root(&Number::from(q as u64), prec).at(span)?;
                    let r = self.mode_num(r);
                    r.pow(&Number::from(p), prec).at(span)
                }
                None => Err(not_real(span)),
            };
        }
        let r = a.pow(b, prec).at(span)?;
        self.check_real(a.is_real() && b.is_real(), r, span)
    }

    /// In real mode, a complex result from real inputs is an error.
    pub(crate) fn check_real(
        &self,
        inputs_real: bool,
        r: Number,
        span: Span,
    ) -> EvalResult<Number> {
        if !self.opts.complex && inputs_real && r.is_complex() {
            Err(not_real(span))
        } else {
            Ok(r)
        }
    }

    pub(crate) fn negate(&mut self, v: Value, span: Span) -> EvalResult<Value> {
        self.map_num(&v, span, &|n| Ok(-n))
    }

    /// Applies a numeric function to a number, or element-wise to lists,
    /// points and matrices.
    pub(crate) fn map_num(
        &mut self,
        v: &Value,
        span: Span,
        f: &dyn Fn(&Number) -> Result<Number, NumberError>,
    ) -> EvalResult<Value> {
        match v {
            Value::Number(n) => {
                let r = f(n).at(span)?;
                let r = self.check_real(n.is_real(), r, span)?;
                Ok(Value::Number(r))
            }
            Value::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.map_num(it, span, f)?);
                }
                Ok(Value::List(out))
            }
            Value::Tuple(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.map_num(it, span, f)?);
                }
                Ok(Value::Tuple(out))
            }
            Value::Matrix(m) => self.mat_map(m, |ev, x| {
                let r = f(x).at(span)?;
                ev.check_real(x.is_real(), r, span)
            }),
            Value::Undefined => Ok(Value::Undefined),
            other => Err(EvalError::ty(
                format!("expected a number, got a {}", other.type_name()),
                span,
            )),
        }
    }

    /// Two-argument numeric function with broadcasting over lists.
    pub(crate) fn zip_num(
        &mut self,
        a: &Value,
        b: &Value,
        span: Span,
        f: &dyn Fn(&Number, &Number) -> Result<Number, NumberError>,
    ) -> EvalResult<Value> {
        match (a, b) {
            (Value::Undefined, _) | (_, Value::Undefined) => Ok(Value::Undefined),
            (Value::Number(x), Value::Number(y)) => {
                let r = f(x, y).at(span)?;
                let r = self.check_real(x.is_real() && y.is_real(), r, span)?;
                Ok(Value::Number(r))
            }
            (Value::List(xs), Value::List(ys)) => {
                if xs.len() != ys.len() {
                    return Err(EvalError::other("list lengths differ", span));
                }
                let mut out = Vec::with_capacity(xs.len());
                for (x, y) in xs.iter().zip(ys) {
                    out.push(self.zip_num(x, y, span, f)?);
                }
                Ok(Value::List(out))
            }
            (Value::List(xs), y) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.zip_num(x, y, span, f)?);
                }
                Ok(Value::List(out))
            }
            (x, Value::List(ys)) => {
                let mut out = Vec::with_capacity(ys.len());
                for y in ys {
                    out.push(self.zip_num(x, y, span, f)?);
                }
                Ok(Value::List(out))
            }
            (a, b) => Err(EvalError::ty(
                format!(
                    "expected numbers, got a {} and a {}",
                    a.type_name(),
                    b.type_name()
                ),
                span,
            )),
        }
    }

    // ----- matrices ------------------------------------------------------

    fn mat_map(
        &mut self,
        m: &Matrix,
        mut f: impl FnMut(&mut Self, &Number) -> EvalResult<Number>,
    ) -> EvalResult<Value> {
        let mut data = Vec::with_capacity(m.data.len());
        for x in &m.data {
            data.push(f(self, x)?);
        }
        Ok(Value::Matrix(Matrix {
            rows: m.rows,
            cols: m.cols,
            data,
        }))
    }

    fn mat_mat(&mut self, op: Arith, m: &Matrix, n: &Matrix, span: Span) -> EvalResult<Value> {
        match op {
            Arith::Add | Arith::Sub => {
                if m.rows != n.rows || m.cols != n.cols {
                    return Err(EvalError::other(
                        format!(
                            "matrix sizes differ ({}×{} and {}×{})",
                            m.rows, m.cols, n.rows, n.cols
                        ),
                        span,
                    ));
                }
                let data = m
                    .data
                    .iter()
                    .zip(&n.data)
                    .map(|(a, b)| if op == Arith::Add { a + b } else { a - b })
                    .collect();
                Ok(Value::Matrix(Matrix {
                    rows: m.rows,
                    cols: m.cols,
                    data,
                }))
            }
            Arith::Mul => mat_mul(m, n, span).map(Value::Matrix),
            _ => Err(EvalError::ty(
                format!("cannot {} two matrices", op.verb()),
                span,
            )),
        }
    }

    fn mat_vec(&mut self, m: &Matrix, v: &[Value], span: Span) -> EvalResult<Value> {
        if v.len() != m.cols {
            return Err(EvalError::other(
                format!(
                    "cannot multiply a {}×{} matrix by a vector of length {}",
                    m.rows,
                    m.cols,
                    v.len()
                ),
                span,
            ));
        }
        let mut nums = Vec::with_capacity(v.len());
        for x in v {
            nums.push(self.expect_num(x.clone(), span)?);
        }
        let mut out = Vec::with_capacity(m.rows);
        for r in 0..m.rows {
            let mut acc = Number::zero();
            for (a, b) in m.row(r).iter().zip(&nums) {
                acc = acc + a * b;
            }
            out.push(Value::Number(acc));
        }
        Ok(Value::List(out))
    }

    fn mat_pow(&mut self, m: &Matrix, e: &Number, span: Span) -> EvalResult<Value> {
        if m.rows != m.cols {
            return Err(EvalError::other("only square matrices have powers", span));
        }
        let n = e
            .integer_value()
            .ok()
            .and_then(|n| n.to_u32())
            .ok_or_else(|| {
                EvalError::unsupported(
                    "matrix powers need a non-negative integer exponent (inverse not supported yet)",
                    span,
                )
            })?;
        let mut result = identity(m.rows);
        let mut base = m.clone();
        let mut k = n;
        while k > 0 {
            self.tick(span)?;
            if k & 1 == 1 {
                result = mat_mul(&result, &base, span)?;
            }
            k >>= 1;
            if k > 0 {
                base = mat_mul(&base, &base, span)?;
            }
        }
        Ok(Value::Matrix(result))
    }

    // ----- comparison and logic -----------------------------------------

    pub(crate) fn compare(
        &mut self,
        a: &Value,
        op: CmpOp,
        b: &Value,
        span: Span,
    ) -> EvalResult<Value> {
        match (a, b) {
            (Value::Undefined, _) | (_, Value::Undefined) => Ok(Value::Undefined),
            (Value::List(xs), Value::List(ys)) => {
                if xs.len() != ys.len() {
                    return Err(EvalError::other("list lengths differ", span));
                }
                let mut out = Vec::with_capacity(xs.len());
                for (x, y) in xs.iter().zip(ys) {
                    out.push(self.compare(x, op, y, span)?);
                }
                Ok(Value::List(out))
            }
            (Value::List(xs), y) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.compare(x, op, y, span)?);
                }
                Ok(Value::List(out))
            }
            (x, Value::List(ys)) => {
                let mut out = Vec::with_capacity(ys.len());
                for y in ys {
                    out.push(self.compare(x, op, y, span)?);
                }
                Ok(Value::List(out))
            }
            (Value::Number(x), Value::Number(y)) => {
                let r = match op {
                    CmpOp::Eq => x == y,
                    CmpOp::Ne => x != y,
                    _ => {
                        let ord = x.cmp_real(y).map_err(|_| {
                            EvalError::new(
                                ErrorKind::NotReal,
                                "complex numbers cannot be ordered",
                                span,
                            )
                        })?;
                        ordering_holds(op, ord)
                    }
                };
                Ok(Value::Bool(r))
            }
            (Value::Str(x), Value::Str(y)) => Ok(Value::Bool(ordering_holds(op, x.cmp(y)))),
            (x, y) if matches!(op, CmpOp::Eq | CmpOp::Ne) => {
                Ok(Value::Bool((x == y) == (op == CmpOp::Eq)))
            }
            (x, y) => Err(EvalError::ty(
                format!("cannot compare a {} and a {}", x.type_name(), y.type_name()),
                span,
            )),
        }
    }

    /// Three-valued `and`/`or` with list broadcasting.
    pub(crate) fn logic2(
        &mut self,
        a: Value,
        b: Value,
        is_and: bool,
        span: Span,
    ) -> EvalResult<Value> {
        match (a, b) {
            (Value::List(xs), Value::List(ys)) => {
                if xs.len() != ys.len() {
                    return Err(EvalError::other("list lengths differ", span));
                }
                let mut out = Vec::with_capacity(xs.len());
                for (x, y) in xs.into_iter().zip(ys) {
                    out.push(self.logic2(x, y, is_and, span)?);
                }
                Ok(Value::List(out))
            }
            (Value::List(xs), y) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.logic2(x, y.clone(), is_and, span)?);
                }
                Ok(Value::List(out))
            }
            (x, Value::List(ys)) => {
                let mut out = Vec::with_capacity(ys.len());
                for y in ys {
                    out.push(self.logic2(x.clone(), y, is_and, span)?);
                }
                Ok(Value::List(out))
            }
            (Value::Bool(x), Value::Bool(y)) => {
                Ok(Value::Bool(if is_and { x && y } else { x || y }))
            }
            // A decisive operand wins over an undefined one.
            (Value::Bool(x), Value::Undefined) | (Value::Undefined, Value::Bool(x))
                if x != is_and =>
            {
                Ok(Value::Bool(x))
            }
            (Value::Bool(_) | Value::Undefined, Value::Bool(_) | Value::Undefined) => {
                Ok(Value::Undefined)
            }
            (x, y) => Err(EvalError::ty(
                format!(
                    "`{}` needs true/false operands, got a {} and a {}",
                    if is_and { "and" } else { "or" },
                    x.type_name(),
                    y.type_name()
                ),
                span,
            )),
        }
    }

    pub(crate) fn logic_not(&mut self, v: Value, span: Span) -> EvalResult<Value> {
        match v {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            Value::Undefined => Ok(Value::Undefined),
            Value::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.logic_not(it, span)?);
                }
                Ok(Value::List(out))
            }
            other => Err(EvalError::ty(
                format!("`not` needs true/false, got a {}", other.type_name()),
                span,
            )),
        }
    }

    // ----- indexing and members -----------------------------------------

    /// `base[idx]`, 1-based. A list of integers selects several elements, a
    /// list of booleans filters.
    pub(crate) fn index(&mut self, base: Value, idx: Value, span: Span) -> EvalResult<Value> {
        match idx {
            Value::Undefined => Ok(Value::Undefined),
            Value::Number(n) => element(&base, &n, span),
            Value::List(items) => {
                if !items.is_empty() && items.iter().all(|v| matches!(v, Value::Bool(_))) {
                    let elems = elements(base, span)?;
                    if elems.len() != items.len() {
                        return Err(EvalError::new(
                            ErrorKind::Index,
                            format!(
                                "filter has {} conditions for {} elements",
                                items.len(),
                                elems.len()
                            ),
                            span,
                        ));
                    }
                    Ok(Value::List(
                        elems
                            .into_iter()
                            .zip(items)
                            .filter(|(_, keep)| *keep == Value::Bool(true))
                            .map(|(v, _)| v)
                            .collect(),
                    ))
                } else {
                    let mut out = Vec::with_capacity(items.len());
                    for i in items {
                        out.push(self.index(base.clone(), i, span)?);
                    }
                    Ok(Value::List(out))
                }
            }
            other => Err(EvalError::new(
                ErrorKind::Index,
                format!(
                    "an index must be a number or a list, got a {}",
                    other.type_name()
                ),
                span,
            )),
        }
    }

    /// Member value if `name` is a member of `v` (not a method).
    pub(crate) fn try_member(&self, v: &Value, name: &str) -> Option<Value> {
        match v {
            Value::Tuple(items) => {
                let k = match name {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    "w" => 3,
                    _ => return None,
                };
                items.get(k).cloned()
            }
            Value::Number(n) => match name {
                "re" => Some(Value::Number(n.re())),
                "im" => Some(Value::Number(n.im())),
                _ => None,
            },
            Value::Matrix(m) => match name {
                "T" => Some(Value::Matrix(m.transpose())),
                "rows" => Some(Value::num(Number::from(m.rows as u64))),
                "cols" => Some(Value::num(Number::from(m.cols as u64))),
                _ => None,
            },
            Value::List(items) if !items.is_empty() => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.try_member(it, name)?);
                }
                Some(Value::List(out))
            }
            _ => None,
        }
    }

    pub(crate) fn member(&mut self, v: Value, name: &str, span: Span) -> EvalResult<Value> {
        self.try_member(&v, name).ok_or_else(|| {
            EvalError::ty(format!("a {} has no member `{name}`", v.type_name()), span)
        })
    }
}

fn ordering_holds(op: CmpOp, ord: Ordering) -> bool {
    match op {
        CmpOp::Lt => ord == Ordering::Less,
        CmpOp::Le => ord != Ordering::Greater,
        CmpOp::Gt => ord == Ordering::Greater,
        CmpOp::Ge => ord != Ordering::Less,
        CmpOp::Eq => ord == Ordering::Equal,
        CmpOp::Ne => ord != Ordering::Equal,
    }
}

/// `p/q` with odd `q` for a fractional exponent (exact rationals, or
/// floats within 1e-9 of a fraction with a small odd denominator).
fn odd_root_exponent(b: &Number) -> Option<(i64, u32)> {
    if let Some(r) = b.to_rational() {
        let q = r.denom().to_u32()?;
        let p = r.numer().to_i64()?;
        return (q % 2 == 1).then_some((p, q));
    }
    let x = b.to_f64();
    for q in (3..100u32).step_by(2) {
        let t = x * q as f64;
        let p = t.round();
        if (t - p).abs() < 1e-9 * t.abs().max(1.0) && p.abs() < 1e15 {
            return Some((p as i64, q));
        }
    }
    None
}

fn identity(n: usize) -> Matrix {
    let mut data = vec![Number::zero(); n * n];
    for i in 0..n {
        data[i * n + i] = Number::one();
    }
    Matrix {
        rows: n,
        cols: n,
        data,
    }
}

fn mat_mul(m: &Matrix, n: &Matrix, span: Span) -> EvalResult<Matrix> {
    if m.cols != n.rows {
        return Err(EvalError::other(
            format!(
                "cannot multiply a {}×{} matrix by a {}×{} matrix",
                m.rows, m.cols, n.rows, n.cols
            ),
            span,
        ));
    }
    let mut data = Vec::with_capacity(m.rows * n.cols);
    for r in 0..m.rows {
        for c in 0..n.cols {
            let mut acc = Number::zero();
            for k in 0..m.cols {
                acc = acc + m.get(r, k) * n.get(k, c);
            }
            data.push(acc);
        }
    }
    Ok(Matrix {
        rows: m.rows,
        cols: n.cols,
        data,
    })
}

/// Elements of an indexable value.
fn elements(base: Value, span: Span) -> EvalResult<Vec<Value>> {
    match base {
        Value::List(items) | Value::Tuple(items) => Ok(items),
        Value::Matrix(m) => Ok(m.to_rows()),
        Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
        other => Err(EvalError::new(
            ErrorKind::Index,
            format!("a {} cannot be indexed", other.type_name()),
            span,
        )),
    }
}

fn element(base: &Value, n: &Number, span: Span) -> EvalResult<Value> {
    let len = match base {
        Value::List(items) | Value::Tuple(items) => items.len(),
        Value::Matrix(m) => m.rows,
        Value::Str(s) => s.chars().count(),
        Value::Undefined => return Ok(Value::Undefined),
        other => {
            return Err(EvalError::new(
                ErrorKind::Index,
                format!("a {} cannot be indexed", other.type_name()),
                span,
            ));
        }
    };
    let i = n
        .integer_value()
        .ok()
        .and_then(|i| i.to_usize())
        .filter(|&i| i >= 1)
        .ok_or_else(|| {
            EvalError::new(
                ErrorKind::Index,
                format!("index must be a positive integer, got {n}"),
                span,
            )
        })?;
    if i > len {
        return Err(EvalError::new(
            ErrorKind::Index,
            format!("index {i} out of range (length {len})"),
            span,
        ));
    }
    Ok(match base {
        Value::List(items) | Value::Tuple(items) => items[i - 1].clone(),
        Value::Matrix(m) => Value::List(m.row(i - 1).iter().cloned().map(Value::Number).collect()),
        Value::Str(s) => Value::Str(s.chars().nth(i - 1).expect("in range").to_string()),
        _ => unreachable!(),
    })
}
