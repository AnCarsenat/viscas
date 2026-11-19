//! Abstract syntax tree and its canonical pretty-printer.
//!
//! Every [`Expr`] and [`Stmt`] carries the byte [`Span`] it was parsed from.
//! The [`Display`](fmt::Display) implementations print *canonical* text:
//! minimal parentheses, explicit `*`, ASCII operators (`<=`, `!=`, `==`),
//! `sqrt(x)` for `√x`, and `∫ ∑ ∏` for calculus operators. Re-parsing the
//! printed text yields the same tree up to spans (see [`Expr::without_spans`]).

use std::fmt::{self, Write as _};

use super::token::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `+x`
    Pos,
    /// `not x`
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    And,
    Or,
    /// Unit conversion `a to b`.
    To,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostfixOp {
    /// `n!`
    Factorial,
    /// `x%`
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl CmpOp {
    pub fn as_str(self) -> &'static str {
        match self {
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BigOpKind {
    Sum,
    Product,
}

/// `name = iter` in a list comprehension.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub name: String,
    pub iter: Expr,
    pub span: Span,
}

/// `cond: value` in a piecewise expression. `value` is `None` only for the
/// condition-only form `{x > 0}` (value 1 where the condition holds).
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub cond: Expr,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    /// Numeric literal; `text` is the raw source text (without the `i`
    /// suffix), interpreted exactly by a later pass.
    Number {
        text: String,
        imaginary: bool,
    },
    Str(String),
    Ident(String),
    /// `f'`, `f''` — `order` primes applied to `expr`.
    Prime {
        expr: Box<Expr>,
        order: u32,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Postfix {
        op: PostfixOp,
        expr: Box<Expr>,
    },
    /// Comparison chain `first op1 e1 op2 e2 ...` (`0 < x < 2`).
    Compare {
        first: Box<Expr>,
        rest: Vec<(CmpOp, Expr)>,
    },
    /// `callee(args)`; also produced for paren-less application `sin x`.
    /// `x(y)` always parses as a call — a later pass may reinterpret it as
    /// multiplication when `x` is not a function.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `expr.name`
    Member {
        expr: Box<Expr>,
        name: String,
    },
    /// `expr[index]`; `index` is a list/range for `L[1, 3]` / `L[2..4]`, a
    /// condition for filters `L[L > 3]`, or a plain expression.
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    /// `[a, b, c]`; matrices are lists of lists.
    List(Vec<Expr>),
    /// `[start..end]` or `[start, second..end]`.
    Range {
        start: Box<Expr>,
        second: Option<Box<Expr>>,
        end: Box<Expr>,
    },
    /// `[body for k = iter, j = iter2]`
    Comprehension {
        body: Box<Expr>,
        bindings: Vec<Binding>,
    },
    /// `(a, b, ...)` with at least two elements.
    Tuple(Vec<Expr>),
    /// `{c1: v1, c2: v2, otherwise}`
    Piecewise {
        branches: Vec<Branch>,
        otherwise: Option<Box<Expr>>,
    },
    /// `expr {cond, ...}` — a domain restriction.
    Restrict {
        expr: Box<Expr>,
        conds: Vec<Expr>,
    },
    /// `d/dx expr`, `d^2/dx^2 expr`
    Derivative {
        var: String,
        order: u32,
        expr: Box<Expr>,
    },
    /// `∫(lo, hi) body dx` or indefinite `∫ body dx`.
    Integral {
        bounds: Option<(Box<Expr>, Box<Expr>)>,
        body: Box<Expr>,
        var: String,
    },
    /// `∑(k = from, to) body` / `∏(...)`.
    BigOp {
        op: BigOpKind,
        var: String,
        from: Box<Expr>,
        to: Box<Expr>,
        body: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ElseBranch {
    Block(Block),
    /// `else if ...` — the statement is always [`StmtKind::If`].
    If(Box<Stmt>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// A bare expression, including inequalities `x^2 + y^2 < 4`.
    Expr(Expr),
    /// `lhs = rhs`: definition (`a = 3`, `f(x) = x^2`) or equation
    /// (`y = sin x`, `x^2 + y^2 = 4`). A later pass classifies it.
    Equation {
        lhs: Expr,
        rhs: Expr,
    },
    /// `lhs ~ rhs`
    Regression {
        lhs: Expr,
        rhs: Expr,
    },
    /// `target -> value`
    Action {
        target: Expr,
        value: Expr,
    },
    /// `let name = value`
    Let {
        name: String,
        value: Expr,
    },
    If {
        cond: Expr,
        then: Block,
        otherwise: Option<ElseBranch>,
    },
    /// `for var in iter { ... }`
    For {
        var: String,
        iter: Expr,
        body: Block,
    },
    While {
        cond: Expr,
        body: Block,
    },
    /// `fn name(params) { ... }`
    Fn {
        name: String,
        params: Vec<String>,
        body: Block,
    },
    Return(Option<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Span stripping (structural equality)
// ---------------------------------------------------------------------------

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Expr { kind, span }
    }

    /// A copy with every span reset to the default, for structural comparison.
    pub fn without_spans(&self) -> Expr {
        let mut e = self.clone();
        e.clear_spans();
        e
    }

    pub fn clear_spans(&mut self) {
        self.span = Span::default();
        match &mut self.kind {
            ExprKind::Number { .. } | ExprKind::Str(_) | ExprKind::Ident(_) => {}
            ExprKind::Prime { expr, .. }
            | ExprKind::Unary { expr, .. }
            | ExprKind::Postfix { expr, .. }
            | ExprKind::Member { expr, .. }
            | ExprKind::Derivative { expr, .. } => expr.clear_spans(),
            ExprKind::Binary { lhs, rhs, .. } => {
                lhs.clear_spans();
                rhs.clear_spans();
            }
            ExprKind::Compare { first, rest } => {
                first.clear_spans();
                rest.iter_mut().for_each(|(_, e)| e.clear_spans());
            }
            ExprKind::Call { callee, args } => {
                callee.clear_spans();
                args.iter_mut().for_each(Expr::clear_spans);
            }
            ExprKind::Index { expr, index } => {
                expr.clear_spans();
                index.clear_spans();
            }
            ExprKind::List(items) | ExprKind::Tuple(items) => {
                items.iter_mut().for_each(Expr::clear_spans)
            }
            ExprKind::Range { start, second, end } => {
                start.clear_spans();
                if let Some(s) = second {
                    s.clear_spans();
                }
                end.clear_spans();
            }
            ExprKind::Comprehension { body, bindings } => {
                body.clear_spans();
                for b in bindings {
                    b.span = Span::default();
                    b.iter.clear_spans();
                }
            }
            ExprKind::Piecewise {
                branches,
                otherwise,
            } => {
                for b in branches {
                    b.cond.clear_spans();
                    if let Some(v) = &mut b.value {
                        v.clear_spans();
                    }
                }
                if let Some(o) = otherwise {
                    o.clear_spans();
                }
            }
            ExprKind::Restrict { expr, conds } => {
                expr.clear_spans();
                conds.iter_mut().for_each(Expr::clear_spans);
            }
            ExprKind::Integral { bounds, body, .. } => {
                if let Some((lo, hi)) = bounds {
                    lo.clear_spans();
                    hi.clear_spans();
                }
                body.clear_spans();
            }
            ExprKind::BigOp { from, to, body, .. } => {
                from.clear_spans();
                to.clear_spans();
                body.clear_spans();
            }
        }
    }
}

impl Block {
    pub fn clear_spans(&mut self) {
        self.span = Span::default();
        self.stmts.iter_mut().for_each(Stmt::clear_spans);
    }
}

impl Stmt {
    pub fn without_spans(&self) -> Stmt {
        let mut s = self.clone();
        s.clear_spans();
        s
    }

    pub fn clear_spans(&mut self) {
        self.span = Span::default();
        match &mut self.kind {
            StmtKind::Expr(e) | StmtKind::Let { value: e, .. } => e.clear_spans(),
            StmtKind::Equation { lhs, rhs } | StmtKind::Regression { lhs, rhs } => {
                lhs.clear_spans();
                rhs.clear_spans();
            }
            StmtKind::Action { target, value } => {
                target.clear_spans();
                value.clear_spans();
            }
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                cond.clear_spans();
                then.clear_spans();
                match otherwise {
                    Some(ElseBranch::Block(b)) => b.clear_spans(),
                    Some(ElseBranch::If(s)) => s.clear_spans(),
                    None => {}
                }
            }
            StmtKind::For { iter, body, .. } => {
                iter.clear_spans();
                body.clear_spans();
            }
            StmtKind::While { cond, body } => {
                cond.clear_spans();
                body.clear_spans();
            }
            StmtKind::Fn { body, .. } => body.clear_spans(),
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    e.clear_spans();
                }
            }
        }
    }
}

impl Program {
    pub fn without_spans(&self) -> Program {
        let mut p = self.clone();
        p.span = Span::default();
        p.stmts.iter_mut().for_each(Stmt::clear_spans);
        p
    }
}

// ---------------------------------------------------------------------------
// Pretty-printer
// ---------------------------------------------------------------------------

/// Binding strength levels, loosest first.
mod prec {
    pub const RESTRICT: u8 = 1;
    pub const TO: u8 = 2;
    pub const OR: u8 = 3;
    pub const AND: u8 = 4;
    pub const NOT: u8 = 5;
    pub const CMP: u8 = 6;
    pub const ADD: u8 = 7;
    pub const MUL: u8 = 8;
    pub const UNARY: u8 = 9;
    pub const POW: u8 = 10;
    pub const POSTFIX: u8 = 11;
    pub const ATOM: u8 = 12;
}

fn precedence(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Restrict { .. } => prec::RESTRICT,
        ExprKind::Binary { op, .. } => match op {
            BinOp::To => prec::TO,
            BinOp::Or => prec::OR,
            BinOp::And => prec::AND,
            BinOp::Add | BinOp::Sub => prec::ADD,
            BinOp::Mul | BinOp::Div | BinOp::Mod => prec::MUL,
            BinOp::Pow => prec::POW,
        },
        ExprKind::Unary {
            op: UnaryOp::Not, ..
        } => prec::NOT,
        ExprKind::Unary { .. } => prec::UNARY,
        ExprKind::Compare { .. } => prec::CMP,
        // Prefix operators whose operand extends over the rest of the product.
        ExprKind::Derivative { .. } | ExprKind::BigOp { .. } => prec::MUL,
        ExprKind::Integral { .. } => prec::UNARY,
        ExprKind::Postfix { .. }
        | ExprKind::Prime { .. }
        | ExprKind::Call { .. }
        | ExprKind::Index { .. }
        | ExprKind::Member { .. } => prec::POSTFIX,
        ExprKind::Number { .. }
        | ExprKind::Str(_)
        | ExprKind::Ident(_)
        | ExprKind::List(_)
        | ExprKind::Range { .. }
        | ExprKind::Comprehension { .. }
        | ExprKind::Tuple(_)
        | ExprKind::Piecewise { .. } => prec::ATOM,
    }
}

/// Greedy prefix operators swallow everything up to the next `+`/`-`, so
/// they need parentheses when something multiplicative follows them.
fn is_greedy(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Derivative { .. } | ExprKind::BigOp { .. })
}

fn child(f: &mut fmt::Formatter<'_>, e: &Expr, min: u8) -> fmt::Result {
    if precedence(e) < min {
        write!(f, "({e})")
    } else {
        write!(f, "{e}")
    }
}

fn comma_list(f: &mut fmt::Formatter<'_>, items: &[Expr]) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

fn write_str_literal(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            c => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Number { text, imaginary } => {
                f.write_str(text)?;
                if *imaginary {
                    f.write_char('i')?;
                }
                Ok(())
            }
            ExprKind::Str(s) => write_str_literal(f, s),
            ExprKind::Ident(name) => f.write_str(name),
            ExprKind::Prime { expr, order } => {
                child(f, expr, prec::POSTFIX)?;
                for _ in 0..*order {
                    f.write_char('\'')?;
                }
                Ok(())
            }
            ExprKind::Unary { op, expr } => {
                let (s, min) = match op {
                    UnaryOp::Neg => ("-", prec::UNARY),
                    UnaryOp::Pos => ("+", prec::UNARY),
                    UnaryOp::Not => ("not ", prec::NOT),
                };
                f.write_str(s)?;
                child(f, expr, min)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (sep, p) = match op {
                    BinOp::Pow => {
                        child(f, lhs, prec::POSTFIX)?;
                        f.write_char('^')?;
                        return child(f, rhs, prec::POW);
                    }
                    BinOp::Add => (" + ", prec::ADD),
                    BinOp::Sub => (" - ", prec::ADD),
                    BinOp::Mul => (" * ", prec::MUL),
                    BinOp::Div => (" / ", prec::MUL),
                    BinOp::Mod => (" mod ", prec::MUL),
                    BinOp::And => (" and ", prec::AND),
                    BinOp::Or => (" or ", prec::OR),
                    BinOp::To => (" to ", prec::TO),
                };
                if p == prec::MUL && is_greedy(lhs) {
                    write!(f, "({lhs})")?;
                } else {
                    child(f, lhs, p)?;
                }
                f.write_str(sep)?;
                child(f, rhs, p + 1)
            }
            ExprKind::Postfix { op, expr } => {
                child(f, expr, prec::POSTFIX)?;
                f.write_char(match op {
                    PostfixOp::Factorial => '!',
                    PostfixOp::Percent => '%',
                })
            }
            ExprKind::Compare { first, rest } => {
                child(f, first, prec::ADD)?;
                for (op, e) in rest {
                    write!(f, " {} ", op.as_str())?;
                    child(f, e, prec::ADD)?;
                }
                Ok(())
            }
            ExprKind::Call { callee, args } => {
                child(f, callee, prec::POSTFIX)?;
                f.write_char('(')?;
                comma_list(f, args)?;
                f.write_char(')')
            }
            ExprKind::Member { expr, name } => {
                child(f, expr, prec::POSTFIX)?;
                write!(f, ".{name}")
            }
            ExprKind::Index { expr, index } => {
                if matches!(expr.kind, ExprKind::Number { .. }) {
                    write!(f, "({expr})")?;
                } else {
                    child(f, expr, prec::POSTFIX)?;
                }
                f.write_char('[')?;
                match &index.kind {
                    ExprKind::List(items) if items.len() != 1 => comma_list(f, items)?,
                    ExprKind::Range { .. } | ExprKind::Comprehension { .. } => {
                        let s = index.to_string();
                        f.write_str(&s[1..s.len() - 1])?;
                    }
                    _ => write!(f, "{index}")?,
                }
                f.write_char(']')
            }
            ExprKind::List(items) => {
                f.write_char('[')?;
                comma_list(f, items)?;
                f.write_char(']')
            }
            ExprKind::Range { start, second, end } => {
                write!(f, "[{start}")?;
                if let Some(s) = second {
                    write!(f, ", {s}")?;
                }
                // Keep `.5` from merging into the dots (`...5`).
                let end = end.to_string();
                let gap = if end.starts_with('.') { " " } else { "" };
                write!(f, "..{gap}{end}]")
            }
            ExprKind::Comprehension { body, bindings } => {
                write!(f, "[{body} for ")?;
                for (i, b) in bindings.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{} = {}", b.name, b.iter)?;
                }
                f.write_char(']')
            }
            ExprKind::Tuple(items) => {
                f.write_char('(')?;
                comma_list(f, items)?;
                f.write_char(')')
            }
            ExprKind::Piecewise {
                branches,
                otherwise,
            } => {
                f.write_char('{')?;
                for (i, b) in branches.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", b.cond)?;
                    if let Some(v) = &b.value {
                        write!(f, ": {v}")?;
                    }
                }
                if let Some(o) = otherwise {
                    write!(f, ", {o}")?;
                }
                f.write_char('}')
            }
            ExprKind::Restrict { expr, conds } => {
                child(f, expr, prec::TO)?;
                f.write_str(" {")?;
                comma_list(f, conds)?;
                f.write_char('}')
            }
            ExprKind::Derivative { var, order, expr } => {
                if *order == 1 {
                    write!(f, "d/d{var} ")?;
                } else {
                    write!(f, "d^{order}/d{var}^{order} ")?;
                }
                child(f, expr, prec::MUL)
            }
            ExprKind::Integral { bounds, body, var } => {
                let mut b = String::new();
                if precedence(body) < prec::ADD {
                    write!(b, "({body})")?;
                } else {
                    write!(b, "{body}")?;
                }
                match bounds {
                    Some((lo, hi)) => write!(f, "∫({lo}, {hi}) {b} d{var}"),
                    // `∫ (a, b) ...` would read as bounds: wrap the integrand.
                    None if b.starts_with('(') => write!(f, "∫ ({b}) d{var}"),
                    None => write!(f, "∫ {b} d{var}"),
                }
            }
            ExprKind::BigOp {
                op,
                var,
                from,
                to,
                body,
            } => {
                let sym = match op {
                    BigOpKind::Sum => '∑',
                    BigOpKind::Product => '∏',
                };
                write!(f, "{sym}({var} = {from}, {to}) ")?;
                child(f, body, prec::MUL)
            }
        }
    }
}

const INDENT: &str = "    ";

fn write_block(f: &mut fmt::Formatter<'_>, block: &Block, depth: usize) -> fmt::Result {
    if block.stmts.is_empty() {
        return f.write_str("{}");
    }
    f.write_str("{\n")?;
    for s in &block.stmts {
        write_stmt(f, s, depth + 1)?;
        f.write_char('\n')?;
    }
    for _ in 0..depth {
        f.write_str(INDENT)?;
    }
    f.write_char('}')
}

/// Conditions of `if`/`while`/`for` are followed by `{`, so a trailing
/// restriction must be parenthesised.
fn write_header_expr(f: &mut fmt::Formatter<'_>, e: &Expr) -> fmt::Result {
    child(f, e, prec::TO)
}

fn write_stmt(f: &mut fmt::Formatter<'_>, stmt: &Stmt, depth: usize) -> fmt::Result {
    for _ in 0..depth {
        f.write_str(INDENT)?;
    }
    write_stmt_body(f, stmt, depth)
}

fn write_stmt_body(f: &mut fmt::Formatter<'_>, stmt: &Stmt, depth: usize) -> fmt::Result {
    match &stmt.kind {
        StmtKind::Expr(e) => write!(f, "{e}"),
        StmtKind::Equation { lhs, rhs } => write!(f, "{lhs} = {rhs}"),
        StmtKind::Regression { lhs, rhs } => write!(f, "{lhs} ~ {rhs}"),
        StmtKind::Action { target, value } => write!(f, "{target} -> {value}"),
        StmtKind::Let { name, value } => write!(f, "let {name} = {value}"),
        StmtKind::If {
            cond,
            then,
            otherwise,
        } => {
            f.write_str("if ")?;
            write_header_expr(f, cond)?;
            f.write_char(' ')?;
            write_block(f, then, depth)?;
            match otherwise {
                Some(ElseBranch::Block(b)) => {
                    f.write_str(" else ")?;
                    write_block(f, b, depth)
                }
                Some(ElseBranch::If(s)) => {
                    f.write_str(" else ")?;
                    write_stmt_body(f, s, depth)
                }
                None => Ok(()),
            }
        }
        StmtKind::For { var, iter, body } => {
            write!(f, "for {var} in ")?;
            write_header_expr(f, iter)?;
            f.write_char(' ')?;
            write_block(f, body, depth)
        }
        StmtKind::While { cond, body } => {
            f.write_str("while ")?;
            write_header_expr(f, cond)?;
            f.write_char(' ')?;
            write_block(f, body, depth)
        }
        StmtKind::Fn { name, params, body } => {
            write!(f, "fn {name}({}) ", params.join(", "))?;
            write_block(f, body, depth)
        }
        StmtKind::Return(None) => f.write_str("return"),
        StmtKind::Return(Some(e)) => write!(f, "return {e}"),
    }
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_stmt(f, self, 0)
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_block(f, self, 0)
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.stmts.iter().enumerate() {
            if i > 0 {
                f.write_char('\n')?;
            }
            write_stmt(f, s, 0)?;
        }
        Ok(())
    }
}
