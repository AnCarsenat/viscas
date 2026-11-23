//! Runtime values and their formatting.

use std::fmt;
use std::sync::Arc;

use super::builtins::Builtin;
use crate::number::Number;
use crate::syntax::{Block, Expr};

/// A runtime value.
///
/// The enum is `#[non_exhaustive]`: the CAS milestone will add a symbolic
/// variant (`Value::Expr`, an unevaluated/simplified expression tree) so that
/// `d/dx x^2` or `√2` can stay exact. Code outside this crate must keep a
/// wildcard arm.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Value {
    Number(Number),
    Bool(bool),
    Str(String),
    /// `[a, b, c]`; may be heterogeneous (lists of points, of lists, ...).
    List(Vec<Value>),
    /// `(a, b, ...)`: points and vectors of any dimension (≥ 2).
    Tuple(Vec<Value>),
    /// A rectangular numeric matrix (a list literal whose elements are
    /// equal-length lists of numbers).
    Matrix(Matrix),
    /// User function (closure) or builtin.
    Function(Arc<Function>),
    /// Result of a failed restriction or a piecewise without a matching
    /// branch. Propagates through arithmetic.
    Undefined,
}

/// Row-major numeric matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<Number>,
}

impl Matrix {
    pub fn get(&self, r: usize, c: usize) -> &Number {
        &self.data[r * self.cols + c]
    }

    pub fn row(&self, r: usize) -> &[Number] {
        &self.data[r * self.cols..(r + 1) * self.cols]
    }

    pub fn transpose(&self) -> Matrix {
        let mut data = Vec::with_capacity(self.data.len());
        for c in 0..self.cols {
            for r in 0..self.rows {
                data.push(self.get(r, c).clone());
            }
        }
        Matrix {
            rows: self.cols,
            cols: self.rows,
            data,
        }
    }

    /// Builds a matrix from a list of rows if every row is a list of the
    /// same non-zero length containing only numbers.
    pub fn from_rows(rows: &[Value]) -> Option<Matrix> {
        let first = match rows.first()? {
            Value::List(r) => r.len(),
            _ => return None,
        };
        if first == 0 {
            return None;
        }
        let mut data = Vec::with_capacity(first * rows.len());
        for row in rows {
            let Value::List(items) = row else { return None };
            if items.len() != first {
                return None;
            }
            for v in items {
                let Value::Number(n) = v else { return None };
                data.push(n.clone());
            }
        }
        Some(Matrix {
            rows: rows.len(),
            cols: first,
            data,
        })
    }

    /// The rows as a list of lists.
    pub fn to_rows(&self) -> Vec<Value> {
        (0..self.rows)
            .map(|r| Value::List(self.row(r).iter().cloned().map(Value::Number).collect()))
            .collect()
    }
}

/// A callable value.
#[derive(Debug)]
pub enum Function {
    Builtin(&'static Builtin),
    User(UserFn),
}

/// A user-defined function: `f(x) = x^2` or `fn f(x) { ... }`.
#[derive(Debug, Clone)]
pub struct UserFn {
    /// `None` for anonymous functions.
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body: FnBody,
    /// Local variables visible where the function was defined (lexical
    /// capture by value). Global names are looked up at call time, so a
    /// global function sees later redefinitions of the names it uses.
    /// Shared (`Arc`) so calls and copies of the closure never clone it.
    pub captured: Arc<Vec<(String, Value)>>,
}

/// Body of a user function.
#[derive(Debug, Clone)]
pub enum FnBody {
    Expr(Expr),
    Block(Block),
}

impl Function {
    pub fn name(&self) -> &str {
        match self {
            Function::Builtin(b) => b.name,
            Function::User(u) => u.name.as_deref().unwrap_or("<anonymous>"),
        }
    }
}

impl Value {
    pub fn num(n: impl Into<Number>) -> Value {
        Value::Number(n.into())
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Tuple(_) => "point",
            Value::Matrix(_) => "matrix",
            Value::Function(_) => "function",
            Value::Undefined => "undefined",
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Value::Number(n) => Some(n),
            _ => None,
        }
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    /// `f64` view of a real number (NaN otherwise). Used by the plot layer.
    pub fn to_f64(&self) -> f64 {
        match self {
            Value::Number(n) => n.to_f64(),
            _ => f64::NAN,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::List(a), Value::List(b)) | (Value::Tuple(a), Value::Tuple(b)) => a == b,
            (Value::Matrix(a), Value::Matrix(b)) => a == b,
            (Value::Function(a), Value::Function(b)) => Arc::ptr_eq(a, b),
            (Value::Undefined, Value::Undefined) => true,
            _ => false,
        }
    }
}

impl From<Number> for Value {
    fn from(n: Number) -> Self {
        Value::Number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

/// How numbers are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberStyle {
    /// Exact values exactly (`1/3`, `2 + 3i`); inexact values as decimals
    /// marked with `≈` (`≈3.14159265359`), so numeric results (π, `∫`,
    /// `d/dx`, `f'`) never look exact.
    Exact,
    /// Everything as decimals (`0.3333333333`).
    Approx,
}

/// Options for [`format_value`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatOptions {
    pub style: NumberStyle,
    /// Significant digits for decimal output.
    pub digits: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            style: NumberStyle::Exact,
            digits: 12,
        }
    }
}

impl FormatOptions {
    pub fn approx(digits: usize) -> Self {
        FormatOptions {
            style: NumberStyle::Approx,
            digits,
        }
    }
}

/// Formats a value: `1/3` (exact) or `0.333333333333` (approx), lists as
/// `[1, 2]`, points as `(1, 2)`, matrices as `[[1, 2], [3, 4]]`.
pub fn format_value(value: &Value, opts: &FormatOptions) -> String {
    let mut out = String::new();
    write_value(&mut out, value, opts);
    out
}

fn format_number(n: &Number, opts: &FormatOptions) -> String {
    match opts.style {
        NumberStyle::Exact if n.is_exact() => n.to_string(),
        // Exactness is never claimed for floats (π, numeric ∫, d/dx ...).
        NumberStyle::Exact => format!("≈{}", n.to_decimal_string(opts.digits)),
        NumberStyle::Approx => n.to_decimal_string(opts.digits),
    }
}

fn write_seq(out: &mut String, items: &[Value], open: char, close: char, opts: &FormatOptions) {
    out.push(open);
    for (i, v) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_value(out, v, opts);
    }
    out.push(close);
}

fn write_value(out: &mut String, value: &Value, opts: &FormatOptions) {
    match value {
        Value::Number(n) => out.push_str(&format_number(n, opts)),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Str(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        Value::List(items) => write_seq(out, items, '[', ']', opts),
        Value::Tuple(items) => write_seq(out, items, '(', ')', opts),
        Value::Matrix(m) => write_seq(out, &m.to_rows(), '[', ']', opts),
        Value::Function(f) => match &**f {
            Function::Builtin(b) => {
                out.push_str("builtin ");
                out.push_str(b.name);
            }
            Function::User(u) => {
                let name = u.name.as_deref().unwrap_or("");
                let params = u.params.join(", ");
                match &u.body {
                    FnBody::Expr(e) => out.push_str(&format!("{name}({params}) = {e}")),
                    FnBody::Block(_) => out.push_str(&format!("fn {name}({params}) {{ … }}")),
                }
            }
        },
        Value::Undefined => out.push_str("undefined"),
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_value(self, &FormatOptions::default()))
    }
}
