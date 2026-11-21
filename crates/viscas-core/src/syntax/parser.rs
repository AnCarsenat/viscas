//! Hand-written recursive-descent parser.
//!
//! See the [module docs](super) for the grammar and the implicit
//! multiplication rules.

use super::ast::*;
use super::token::{Keyword, Span, Token, TokenKind as T};
use super::{DefinitionOp, ParseError, ParseOptions};

type PResult<X> = Result<X, ParseError>;

/// Maximum nesting depth (brackets, prefix operators, blocks, …) and
/// maximum length of non-flattened left-assoc chains (`mod`, `to`).
/// Together with flat n-ary sums/products this bounds the depth of every
/// tree the parser builds, so printing and dropping stay shallow too.
/// Chosen so that the deepest accepted input parses, prints and drops
/// comfortably on a 2 MiB thread stack even in unoptimised builds.
pub const MAX_DEPTH: usize = 64;

/// Functions that may be applied without parentheses (`sin x`, `ln 2x`).
/// Identifiers starting with `log_` (`log_2 x`) are included as well.
pub const PREFIX_FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "csc", "sec", "cot", "arcsin", "arccos", "arctan", "arccsc", "arcsec",
    "arccot", "asin", "acos", "atan", "sinh", "cosh", "tanh", "csch", "sech", "coth", "arcsinh",
    "arccosh", "arctanh", "arsinh", "arcosh", "artanh", "asinh", "acosh", "atanh", "ln", "log",
    "lg", "exp",
];

const TRIG: &[&str] = &[
    "sin", "cos", "tan", "csc", "sec", "cot", "sinh", "cosh", "tanh", "csch", "sech", "coth",
];

/// A valid left side of a definition: `name` or `f(a, b, ...)` with
/// distinct plain parameter names.
pub fn is_definition_head(lhs: &Expr) -> bool {
    match &*lhs.kind {
        ExprKind::Ident(_) => true,
        ExprKind::Call { callee, args } => {
            matches!(&*callee.kind, ExprKind::Ident(_))
                && args.iter().enumerate().all(|(i, a)| match &*a.kind {
                    ExprKind::Ident(p) => !args[..i]
                        .iter()
                        .any(|b| matches!(&*b.kind, ExprKind::Ident(q) if q == p)),
                    _ => false,
                })
        }
        _ => false,
    }
}

/// In [`DefinitionOp::Eq`] mode, `lhs = rhs` is a definition when `lhs` is a
/// definition head whose name is not a plot coordinate (`x`, `y`, `r`) or a
/// prefix function (`sin(x) = y` stays a relation).
fn is_eq_mode_definition(lhs: &Expr) -> bool {
    let name = match &*lhs.kind {
        ExprKind::Ident(n) => n,
        ExprKind::Call { callee, .. } => match &*callee.kind {
            ExprKind::Ident(n) => n,
            _ => return false,
        },
        _ => return false,
    };
    is_definition_head(lhs)
        && !matches!(name.as_str(), "x" | "y" | "r")
        && !is_prefix_function(name)
}

pub fn is_prefix_function(name: &str) -> bool {
    PREFIX_FUNCTIONS.contains(&name) || (name.starts_with("log_") && name.len() > 4)
}

/// `dx`, `dθ`, `dt_1`: the letter `d` followed by exactly one letter and an
/// optional subscript. Returns the variable name (`x`, `θ`, `t_1`).
pub fn differential_var(name: &str) -> Option<&str> {
    let rest = name.strip_prefix('d')?;
    let mut chars = rest.chars();
    let c = chars.next()?;
    if !c.is_alphabetic() {
        return None;
    }
    let tail = chars.as_str();
    if tail.is_empty() || (tail.starts_with('_') && tail.len() > 1) {
        Some(rest)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug)]
struct Ctx {
    /// Inside `()`, `[]`, `||` or expression braces: newlines are
    /// insignificant.
    group: bool,
    /// `=` is an equality comparison (otherwise it ends the expression).
    allow_eq: bool,
    /// A trailing `{...}` is an expression-level restriction. Off at
    /// statement level (restrictions belong to the statement) and in
    /// `if`/`while`/`for` heads (the `{` starts the body).
    allow_restrict: bool,
    /// Parsing an integrand: `dx`-style identifiers end the expression.
    in_integral: bool,
    /// Directly inside `|...|`: a `|` in operator position closes it.
    in_abs: bool,
}

const STMT_CTX: Ctx = Ctx {
    group: false,
    allow_eq: false,
    allow_restrict: false,
    in_integral: false,
    in_abs: false,
};

const EXPR_CTX: Ctx = Ctx {
    allow_restrict: true,
    ..STMT_CTX
};

const GROUP_CTX: Ctx = Ctx {
    group: true,
    allow_eq: true,
    allow_restrict: true,
    in_integral: false,
    in_abs: false,
};

pub(crate) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    ctx: Ctx,
    depth: usize,
    prev: Option<usize>,
    prev_end: usize,
    pub(crate) errors: Vec<ParseError>,
    opts: ParseOptions,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<Token>, opts: ParseOptions) -> Self {
        debug_assert!(matches!(tokens.last().map(|t| &t.kind), Some(T::Eof)));
        Parser {
            tokens,
            pos: 0,
            ctx: STMT_CTX,
            depth: 0,
            prev: None,
            prev_end: 0,
            errors: Vec::new(),
            opts,
        }
    }

    // ------------------------------------------------------------------
    // Entry points
    // ------------------------------------------------------------------

    pub(crate) fn expr_entry(&mut self) -> PResult<Expr> {
        self.skip_newlines();
        let e = self.with_ctx(EXPR_CTX, |p| p.parse_expr())?;
        self.skip_newlines();
        self.expect_eof()?;
        Ok(e)
    }

    pub(crate) fn statement_entry(&mut self) -> PResult<Stmt> {
        self.skip_separators();
        let s = self.parse_stmt()?;
        self.skip_separators();
        self.expect_eof()?;
        Ok(s)
    }

    pub(crate) fn program(&mut self) -> Program {
        let stmts = self.stmt_list(false);
        let end = self.tokens.last().map_or(0, |t| t.span.end);
        Program {
            stmts,
            span: Span::new(0, end),
        }
    }

    fn expect_eof(&mut self) -> PResult<()> {
        if matches!(self.peek(), T::Eof) {
            Ok(())
        } else {
            Err(self.unexpected("end of input"))
        }
    }

    // ------------------------------------------------------------------
    // Token helpers
    // ------------------------------------------------------------------

    /// Index of the current token, skipping newlines inside groups.
    fn cur(&self) -> usize {
        self.skip_from(self.pos)
    }

    /// First token index at or after `i` that is not an insignificant newline.
    fn skip_from(&self, mut i: usize) -> usize {
        if self.ctx.group {
            while matches!(self.tokens[i].kind, T::Newline) {
                i += 1;
            }
        }
        i
    }

    fn peek(&self) -> &T {
        &self.tokens[self.cur()].kind
    }

    fn peek_span(&self) -> Span {
        self.tokens[self.cur()].span
    }

    /// The `n`-th upcoming token (0 = current), skipping newlines in groups.
    fn look(&self, n: usize) -> &T {
        let mut i = self.cur();
        for _ in 0..n {
            if matches!(self.tokens[i].kind, T::Eof) {
                break;
            }
            i = self.skip_from(i + 1);
        }
        &self.tokens[i].kind
    }

    fn prev_kind(&self) -> Option<&T> {
        self.prev.map(|i| &self.tokens[i].kind)
    }

    fn bump(&mut self) -> Token {
        self.pos = self.cur();
        let tok = self.tokens[self.pos].clone();
        if !matches!(tok.kind, T::Eof) {
            self.prev = Some(self.pos);
            self.prev_end = tok.span.end;
            self.pos += 1;
        }
        tok
    }

    fn at(&self, kind: &T) -> bool {
        self.peek() == kind
    }

    fn eat(&mut self, kind: &T) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: T) -> PResult<Token> {
        if self.at(&kind) {
            Ok(self.bump())
        } else {
            Err(self.unexpected(&kind.to_string()))
        }
    }

    fn expect_ident(&mut self) -> PResult<(String, Span)> {
        if let T::Ident(name) = self.peek().clone() {
            let tok = self.bump();
            Ok((name, tok.span))
        } else {
            Err(self.unexpected("identifier"))
        }
    }

    fn unexpected(&self, expected: &str) -> ParseError {
        let span = self.peek_span();
        let found = self.peek().clone();
        ParseError::new(span, format!("expected {expected}, found {found}"))
    }

    fn skip_newlines(&mut self) {
        while matches!(self.tokens[self.pos].kind, T::Newline) {
            self.pos += 1;
        }
    }

    fn skip_separators(&mut self) {
        while matches!(self.tokens[self.pos].kind, T::Newline | T::Semi) {
            self.pos += 1;
        }
    }

    fn with_ctx<R>(&mut self, ctx: Ctx, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.ctx;
        self.ctx = ctx;
        let r = f(self);
        self.ctx = saved;
        r
    }

    /// Go one nesting level deeper, failing past [`MAX_DEPTH`]. The caller
    /// decrements `depth` when done.
    fn enter(&mut self) -> PResult<()> {
        if self.depth >= MAX_DEPTH {
            return Err(self.too_deep());
        }
        self.depth += 1;
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn too_deep(&self) -> ParseError {
        ParseError::new(
            self.peek_span(),
            format!("expression nested too deeply (limit {MAX_DEPTH})"),
        )
    }

    /// Run `f` one nesting level deeper, failing past [`MAX_DEPTH`].
    fn nested<R>(&mut self, f: impl FnOnce(&mut Self) -> PResult<R>) -> PResult<R> {
        self.enter()?;
        let r = f(self);
        self.depth -= 1;
        r
    }

    fn node(&self, kind: ExprKind, start: usize) -> Expr {
        Expr::new(kind, Span::new(start, self.prev_end))
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    fn stmt_list(&mut self, in_block: bool) -> Vec<Stmt> {
        let mut out = Vec::new();
        loop {
            self.skip_separators();
            match self.peek() {
                T::Eof => break,
                T::RBrace if in_block => break,
                T::RBrace => {
                    let tok = self.bump();
                    self.errors.push(ParseError::new(
                        tok.span,
                        "unexpected `}` without matching `{`",
                    ));
                    continue;
                }
                _ => {}
            }
            let start = self.pos;
            let saved = self.ctx;
            match self.parse_stmt() {
                Ok(s) => out.push(s),
                Err(e) => {
                    self.errors.push(e);
                    self.ctx = saved;
                    self.synchronize();
                    if self.pos == start {
                        self.bump();
                    }
                }
            }
        }
        out
    }

    /// Skip to the end of the current statement: a newline, a `;` outside
    /// brackets, or a `}` that closes the enclosing block.
    fn synchronize(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.tokens[self.pos].kind {
                T::Eof | T::Newline => return,
                T::Semi | T::RBrace if depth == 0 => return,
                T::LParen | T::LBracket | T::LBrace => depth += 1,
                T::RParen | T::RBracket | T::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.pos += 1;
        }
    }

    fn expect_terminator(&mut self) -> PResult<()> {
        match self.peek() {
            T::Newline | T::Semi | T::Eof | T::RBrace => Ok(()),
            _ => Err(self.unexpected("end of statement")),
        }
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        let stmt = self.with_ctx(STMT_CTX, |p| p.nested(|p| p.parse_stmt_inner()))?;
        self.expect_terminator()?;
        Ok(stmt)
    }

    /// Trailing `{cond, ...}{...}` groups of a statement.
    fn parse_restrictions(&mut self) -> PResult<Vec<Expr>> {
        let mut conds = Vec::new();
        while self.at(&T::LBrace) {
            self.parse_brace_conds(&mut conds)?;
        }
        Ok(conds)
    }

    fn parse_brace_conds(&mut self, conds: &mut Vec<Expr>) -> PResult<()> {
        let open = self.bump();
        self.with_ctx(GROUP_CTX, |p| {
            if p.at(&T::RBrace) {
                return Err(ParseError::new(
                    open.span.to(p.peek_span()),
                    "empty restriction `{}`",
                ));
            }
            loop {
                conds.push(p.parse_expr()?);
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.expect(T::RBrace)?;
            Ok(())
        })
    }

    fn parse_stmt_inner(&mut self) -> PResult<Stmt> {
        let start = self.peek_span().start;
        let kind = match self.peek() {
            T::Keyword(Keyword::Let) => {
                self.bump();
                let (name, _) = self.expect_ident()?;
                self.expect(T::Assign)?;
                let value = self.parse_expr()?;
                StmtKind::Let { name, value }
            }
            T::Keyword(Keyword::If) => return self.parse_if(),
            T::Keyword(Keyword::For) => {
                self.bump();
                let (var, _) = self.expect_ident()?;
                match self.peek() {
                    T::Ident(s) if s == "in" => {
                        self.bump();
                    }
                    _ => return Err(self.unexpected("`in`")),
                }
                let iter = self.parse_header_expr()?;
                let body = self.parse_block()?;
                StmtKind::For { var, iter, body }
            }
            T::Keyword(Keyword::While) => {
                self.bump();
                let cond = self.parse_header_expr()?;
                let body = self.parse_block()?;
                StmtKind::While { cond, body }
            }
            T::Keyword(Keyword::Fn) => {
                self.bump();
                let (name, _) = self.expect_ident()?;
                self.expect(T::LParen)?;
                let params = self.with_ctx(GROUP_CTX, |p| -> PResult<Vec<String>> {
                    let mut params = Vec::new();
                    if !p.at(&T::RParen) {
                        loop {
                            params.push(p.expect_ident()?.0);
                            if !p.eat(&T::Comma) {
                                break;
                            }
                        }
                    }
                    p.expect(T::RParen)?;
                    Ok(params)
                })?;
                let body = self.parse_block()?;
                StmtKind::Fn { name, params, body }
            }
            T::Keyword(Keyword::Return) => {
                self.bump();
                let value = match self.peek() {
                    T::Newline | T::Semi | T::Eof | T::RBrace => None,
                    _ => Some(self.parse_expr()?),
                };
                StmtKind::Return(value)
            }
            _ => {
                let lhs = self.parse_expr()?;
                match self.peek() {
                    T::Assign | T::Tilde => {
                        let is_eq = self.bump().kind == T::Assign;
                        self.skip_newlines();
                        let rhs = self.parse_expr()?;
                        let restrictions = self.parse_restrictions()?;
                        if is_eq
                            && self.opts.definition_op == DefinitionOp::Eq
                            && is_eq_mode_definition(&lhs)
                        {
                            StmtKind::Definition {
                                lhs,
                                rhs,
                                restrictions,
                            }
                        } else if is_eq {
                            StmtKind::Equation {
                                lhs,
                                rhs,
                                restrictions,
                            }
                        } else {
                            StmtKind::Regression {
                                lhs,
                                rhs,
                                restrictions,
                            }
                        }
                    }
                    T::ColonEq => {
                        let op_span = self.bump().span;
                        if !is_definition_head(&lhs) {
                            return Err(ParseError::new(
                                lhs.span.to(op_span),
                                "the left side of `:=` must be a name or `f(x, ...)` with distinct parameter names",
                            ));
                        }
                        self.skip_newlines();
                        let rhs = self.parse_expr()?;
                        let restrictions = self.parse_restrictions()?;
                        StmtKind::Definition {
                            lhs,
                            rhs,
                            restrictions,
                        }
                    }
                    T::Arrow => {
                        self.bump();
                        self.skip_newlines();
                        let mut actions = vec![(lhs, self.parse_expr()?)];
                        while self.eat(&T::Comma) {
                            self.skip_newlines();
                            let target = self.parse_expr()?;
                            self.expect(T::Arrow)?;
                            self.skip_newlines();
                            actions.push((target, self.parse_expr()?));
                        }
                        StmtKind::Action { actions }
                    }
                    _ => {
                        let restrictions = self.parse_restrictions()?;
                        StmtKind::Expr {
                            expr: lhs,
                            restrictions,
                        }
                    }
                }
            }
        };
        Ok(Stmt {
            kind,
            span: Span::new(start, self.prev_end),
        })
    }

    /// Condition / iterable of `if`, `while`, `for`: `=` means equality and
    /// `{` starts the body rather than a restriction.
    fn parse_header_expr(&mut self) -> PResult<Expr> {
        let ctx = Ctx {
            allow_eq: true,
            allow_restrict: false,
            ..self.ctx
        };
        self.with_ctx(ctx, |p| p.parse_expr())
    }

    fn parse_if(&mut self) -> PResult<Stmt> {
        let start = self.bump().span.start;
        let cond = self.parse_header_expr()?;
        let then = self.parse_block()?;
        let save = self.pos;
        self.skip_newlines();
        let otherwise = if self.eat(&T::Keyword(Keyword::Else)) {
            if self.at(&T::Keyword(Keyword::If)) {
                Some(ElseBranch::If(Box::new(self.nested(|p| p.parse_if())?)))
            } else {
                Some(ElseBranch::Block(self.parse_block()?))
            }
        } else {
            self.pos = save;
            None
        };
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then,
                otherwise,
            },
            span: Span::new(start, self.prev_end),
        })
    }

    fn parse_block(&mut self) -> PResult<Block> {
        let open = self.expect(T::LBrace)?;
        let stmts = self.with_ctx(STMT_CTX, |p| p.stmt_list(true));
        if !self.at(&T::RBrace) {
            let span = self.peek_span();
            return Err(ParseError::new(
                open.span.to(span),
                "unclosed block, expected `}`",
            ));
        }
        let close = self.bump();
        Ok(Block {
            stmts,
            span: open.span.to(close.span),
        })
    }

    // ------------------------------------------------------------------
    // Expressions, loosest first
    // ------------------------------------------------------------------

    pub(crate) fn parse_expr(&mut self) -> PResult<Expr> {
        self.enter()?;
        let r = self.parse_to();
        self.depth -= 1;
        match r {
            Ok(e) if self.ctx.allow_restrict && self.at(&T::LBrace) => self.parse_restrict(e),
            r => r,
        }
    }

    #[inline(never)]
    fn parse_restrict(&mut self, e: Expr) -> PResult<Expr> {
        let mut conds = Vec::new();
        while self.at(&T::LBrace) {
            self.parse_brace_conds(&mut conds)?;
        }
        let start = e.span.start;
        Ok(self.node(ExprKind::Restrict { expr: e, conds }, start))
    }

    fn chain_too_long(&self, op: &str) -> ParseError {
        ParseError::new(
            self.peek_span(),
            format!("too many chained `{op}` operators (limit {MAX_DEPTH})"),
        )
    }

    fn parse_to(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_or()?;
        let mut count = 0;
        while self.at(&T::Keyword(Keyword::To)) {
            count += 1;
            if count > MAX_DEPTH {
                return Err(self.chain_too_long("to"));
            }
            self.bump();
            self.skip_newlines();
            let rhs = self.parse_or()?;
            lhs = binary(BinOp::To, lhs, rhs);
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> PResult<Expr> {
        let first = self.parse_and()?;
        if !self.at(&T::Keyword(Keyword::Or)) {
            return Ok(first);
        }
        let start = first.span.start;
        let mut items = vec![first];
        while self.eat(&T::Keyword(Keyword::Or)) {
            self.skip_newlines();
            items.push(self.parse_and()?);
        }
        Ok(self.node(ExprKind::Or(items), start))
    }

    fn parse_and(&mut self) -> PResult<Expr> {
        let first = self.parse_not()?;
        if !self.at(&T::Keyword(Keyword::And)) {
            return Ok(first);
        }
        let start = first.span.start;
        let mut items = vec![first];
        while self.eat(&T::Keyword(Keyword::And)) {
            self.skip_newlines();
            items.push(self.parse_not()?);
        }
        Ok(self.node(ExprKind::And(items), start))
    }

    fn parse_not(&mut self) -> PResult<Expr> {
        if self.at(&T::Keyword(Keyword::Not)) {
            return self.parse_prefix_op(UnaryOp::Not, Self::parse_not);
        }
        self.parse_compare()
    }

    /// `op operand`, one nesting level deeper.
    #[inline(never)]
    fn parse_prefix_op(
        &mut self,
        op: UnaryOp,
        operand: fn(&mut Self) -> PResult<Expr>,
    ) -> PResult<Expr> {
        let start = self.bump().span.start;
        self.enter()?;
        let r = operand(self);
        self.depth -= 1;
        Ok(self.node(ExprKind::Unary { op, expr: r? }, start))
    }

    fn parse_compare(&mut self) -> PResult<Expr> {
        let first = self.parse_additive()?;
        let mut rest = Vec::new();
        loop {
            let op = match self.peek() {
                T::Lt => CmpOp::Lt,
                T::Le => CmpOp::Le,
                T::Gt => CmpOp::Gt,
                T::Ge => CmpOp::Ge,
                T::NotEq => CmpOp::Ne,
                T::EqEq => CmpOp::Eq,
                T::Assign if self.ctx.allow_eq => CmpOp::Eq,
                _ => break,
            };
            self.bump();
            self.skip_newlines();
            rest.push((op, self.parse_additive()?));
        }
        if rest.is_empty() {
            return Ok(first);
        }
        let start = first.span.start;
        Ok(self.node(ExprKind::Compare { first, rest }, start))
    }

    fn parse_additive(&mut self) -> PResult<Expr> {
        let first = self.parse_multiplicative()?;
        let mut terms = vec![Term {
            op: AddOp::Add,
            expr: first,
        }];
        loop {
            let op = match self.peek() {
                T::Plus => AddOp::Add,
                T::Minus => AddOp::Sub,
                _ => break,
            };
            self.bump();
            self.skip_newlines();
            let expr = self.parse_multiplicative()?;
            terms.push(Term { op, expr });
        }
        Ok(collapse_add(terms))
    }

    /// Products: `*`, `/`, `mod` and implicit multiplication, all at one
    /// precedence level, left to right. The right operand of `/` absorbs
    /// following implicit factors (`1/2x` = `1/(2x)`).
    fn parse_multiplicative(&mut self) -> PResult<Expr> {
        let first = self.parse_unary()?;
        let mut factors = vec![Factor {
            op: MulOp::Mul,
            expr: first,
        }];
        let mut mods = 0;
        loop {
            match self.peek() {
                T::Star => {
                    self.bump();
                    self.skip_newlines();
                    let expr = self.parse_unary()?;
                    factors.push(Factor {
                        op: MulOp::Mul,
                        expr,
                    });
                }
                T::Slash => {
                    self.bump();
                    self.skip_newlines();
                    let expr = self.parse_implicit_product()?;
                    factors.push(Factor {
                        op: MulOp::Div,
                        expr,
                    });
                }
                T::Keyword(Keyword::Mod) => {
                    mods += 1;
                    if mods > MAX_DEPTH {
                        return Err(self.chain_too_long("mod"));
                    }
                    self.bump();
                    self.skip_newlines();
                    let rhs = self.parse_unary()?;
                    let lhs = collapse_mul(std::mem::take(&mut factors));
                    factors.push(Factor {
                        op: MulOp::Mul,
                        expr: binary(BinOp::Mod, lhs, rhs),
                    });
                }
                _ if self.starts_implicit_operand() => {
                    let expr = self.parse_power()?;
                    factors.push(Factor {
                        op: MulOp::Mul,
                        expr,
                    });
                }
                _ => break,
            }
        }
        Ok(collapse_mul(factors))
    }

    /// A signed power term followed by implicitly multiplied power terms;
    /// stops at explicit operators. With `stop_at_fn`, also stops before a
    /// prefix function name (`sin x cos x`).
    fn parse_juxtaposed(&mut self, stop_at_fn: bool) -> PResult<Expr> {
        let first = self.parse_unary()?;
        let mut factors = vec![Factor {
            op: MulOp::Mul,
            expr: first,
        }];
        while self.starts_implicit_operand()
            && !(stop_at_fn && matches!(self.peek(), T::Ident(n) if is_prefix_function(n)))
        {
            let expr = self.parse_power()?;
            factors.push(Factor {
                op: MulOp::Mul,
                expr,
            });
        }
        Ok(collapse_mul(factors))
    }

    /// Right operand of `/`.
    fn parse_implicit_product(&mut self) -> PResult<Expr> {
        self.parse_juxtaposed(false)
    }

    /// Argument of a paren-less prefix function or `√`.
    fn parse_fn_arg(&mut self) -> PResult<Expr> {
        self.nested(|p| {
            if p.at(&T::LBrace) {
                return Err(p.brace_group_error());
            }
            p.parse_juxtaposed(true)
        })
    }

    fn brace_group_error(&self) -> ParseError {
        ParseError::new(
            self.peek_span(),
            "braces do not group here; use parentheses, e.g. `x^(2)` or `sqrt(x)`",
        )
    }

    /// Can the next token start an implicitly multiplied operand?
    fn starts_implicit_operand(&self) -> bool {
        let after_number = matches!(self.prev_kind(), Some(T::Number { .. }));
        match self.peek() {
            T::Number { .. } => !after_number,
            T::Ident(name) => !(self.ctx.in_integral && differential_var(name).is_some()),
            T::LParen | T::Sqrt | T::Cbrt | T::Integral | T::Sum | T::Product => true,
            T::Bar => !self.ctx.in_abs,
            T::LBracket => after_number,
            _ => false,
        }
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        match self.peek() {
            T::Minus => self.parse_prefix_op(UnaryOp::Neg, Self::parse_unary),
            T::Plus => self.parse_prefix_op(UnaryOp::Pos, Self::parse_unary),
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> PResult<Expr> {
        let base = self.parse_postfix()?;
        if self.eat(&T::Caret) {
            self.skip_newlines();
            let exp = self.parse_exponent()?;
            return Ok(binary(BinOp::Pow, base, exp));
        }
        Ok(base)
    }

    /// Right operand of `^`: allows a leading sign (`2^-3`), right-assoc.
    fn parse_exponent(&mut self) -> PResult<Expr> {
        match self.peek() {
            T::Minus => self.parse_prefix_op(UnaryOp::Neg, Self::parse_exponent),
            T::Plus => self.parse_prefix_op(UnaryOp::Pos, Self::parse_exponent),
            T::LBrace => Err(self.brace_group_error()),
            _ => {
                self.enter()?;
                let r = self.parse_power();
                self.depth -= 1;
                r
            }
        }
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            let start = e.span.start;
            match self.peek() {
                T::LParen if is_callable(&e) && !matches!(self.prev_kind(), Some(T::RParen)) => {
                    let args = self.parse_call_args()?;
                    e = self.node(ExprKind::Call { callee: e, args }, start);
                }
                T::LBracket if !matches!(self.prev_kind(), Some(T::Number { .. })) => {
                    let open = self.bump();
                    let inner = self
                        .with_ctx(GROUP_CTX, |p| p.nested(|p| p.parse_bracket_body(open.span)))?;
                    let indices = match *inner.kind {
                        ExprKind::List(items) if items.is_empty() => {
                            return Err(ParseError::new(inner.span, "empty index `[]`"));
                        }
                        ExprKind::List(items) => items,
                        kind => vec![Expr::new(kind, inner.span)],
                    };
                    e = self.node(ExprKind::Index { expr: e, indices }, start);
                }
                T::Dot => {
                    self.bump();
                    let (name, _) = self.expect_ident()?;
                    e = self.node(ExprKind::Member { expr: e, name }, start);
                }
                T::Bang | T::Percent => {
                    let op = if self.bump().kind == T::Bang {
                        PostfixOp::Factorial
                    } else {
                        PostfixOp::Percent
                    };
                    e = self.node(ExprKind::Postfix { op, expr: e }, start);
                }
                T::Prime => {
                    self.bump();
                    e = match *e.kind {
                        ExprKind::Prime { expr, order } => self.node(
                            ExprKind::Prime {
                                expr,
                                order: order + 1,
                            },
                            start,
                        ),
                        kind => self.node(
                            ExprKind::Prime {
                                expr: Expr::new(kind, e.span),
                                order: 1,
                            },
                            start,
                        ),
                    };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_call_args(&mut self) -> PResult<Vec<Expr>> {
        self.expect(T::LParen)?;
        self.with_ctx(GROUP_CTX, |p| {
            let mut args = Vec::new();
            if !p.at(&T::RParen) {
                loop {
                    args.push(p.parse_expr()?);
                    if !p.eat(&T::Comma) {
                        break;
                    }
                }
            }
            p.expect(T::RParen)?;
            Ok(args)
        })
    }

    /// After `[`: list, range, or comprehension, up to and including `]`.
    fn parse_bracket_body(&mut self, open: Span) -> PResult<Expr> {
        let start = open.start;
        if self.eat(&T::RBracket) {
            return Ok(self.node(ExprKind::List(Vec::new()), start));
        }
        let first = self.parse_expr()?;
        if self.eat(&T::Keyword(Keyword::For)) {
            let mut bindings = Vec::new();
            loop {
                let (name, name_span) = self.expect_ident()?;
                match self.peek() {
                    T::Assign => {}
                    T::Ident(s) if s == "in" => {}
                    _ => return Err(self.unexpected("`=` in comprehension binding")),
                }
                self.bump();
                let iter = self.parse_expr()?;
                let span = name_span.to(iter.span);
                bindings.push(Binding { name, iter, span });
                if !self.eat(&T::Comma) {
                    break;
                }
            }
            self.expect(T::RBracket)?;
            return Ok(self.node(
                ExprKind::Comprehension {
                    body: first,
                    bindings,
                },
                start,
            ));
        }
        let mut items = vec![first];
        loop {
            if self.eat(&T::Comma) {
                items.push(self.parse_expr()?);
            } else if self.at(&T::DotDot) {
                let dots = self.bump();
                if items.len() > 2 {
                    return Err(ParseError::new(
                        dots.span,
                        "a range takes at most two elements before `..`",
                    ));
                }
                let end = self.parse_expr()?;
                self.expect(T::RBracket)?;
                let mut it = items.into_iter();
                let start_e = it.next().expect("at least one element");
                let second = it.next();
                return Ok(self.node(
                    ExprKind::Range {
                        start: start_e,
                        second,
                        end,
                    },
                    start,
                ));
            } else {
                break;
            }
        }
        self.expect(T::RBracket)?;
        Ok(self.node(ExprKind::List(items), start))
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let tok = self.tokens[self.cur()].clone();
        let start = tok.span.start;
        match tok.kind {
            T::Number { text, imaginary } => {
                self.bump();
                Ok(self.node(ExprKind::Number { text, imaginary }, start))
            }
            T::Str(s) => {
                self.bump();
                Ok(self.node(ExprKind::Str(s), start))
            }
            T::Ident(name) => self.parse_ident_primary(name, tok.span),
            T::LParen => {
                self.bump();
                self.with_ctx(GROUP_CTX, |p| {
                    if p.at(&T::RParen) {
                        let close = p.bump();
                        return Err(ParseError::new(
                            tok.span.to(close.span),
                            "empty parentheses `()`",
                        ));
                    }
                    let first = p.parse_expr()?;
                    let mut items = vec![first];
                    while p.eat(&T::Comma) {
                        items.push(p.parse_expr()?);
                    }
                    p.expect(T::RParen)?;
                    if items.len() == 1 {
                        let mut e = items.pop().expect("one element");
                        e.span = Span::new(start, p.prev_end);
                        Ok(e)
                    } else {
                        Ok(p.node(ExprKind::Tuple(items), start))
                    }
                })
            }
            T::LBracket => {
                self.bump();
                self.with_ctx(GROUP_CTX, |p| p.parse_bracket_body(tok.span))
            }
            T::LBrace => self.parse_piecewise(tok.span),
            T::Bar => {
                self.bump();
                let ctx = Ctx {
                    in_abs: true,
                    ..GROUP_CTX
                };
                let inner = self.with_ctx(ctx, |p| -> PResult<Expr> {
                    let e = p.parse_expr()?;
                    if !p.at(&T::Bar) {
                        return Err(p.unexpected("`|` closing the absolute value"));
                    }
                    p.bump();
                    Ok(e)
                })?;
                let callee = Expr::new(ExprKind::Ident("abs".into()), tok.span);
                Ok(self.node(
                    ExprKind::Call {
                        callee,
                        args: vec![inner],
                    },
                    start,
                ))
            }
            T::Sqrt | T::Cbrt => {
                self.bump();
                let name = if tok.kind == T::Sqrt { "sqrt" } else { "cbrt" };
                let arg = self.parse_fn_arg()?;
                let callee = Expr::new(ExprKind::Ident(name.into()), tok.span);
                Ok(self.node(
                    ExprKind::Call {
                        callee,
                        args: vec![arg],
                    },
                    start,
                ))
            }
            T::Integral => self.nested(|p| p.parse_integral()),
            T::Sum => self.nested(|p| p.parse_bigop(BigOpKind::Sum)),
            T::Product => self.nested(|p| p.parse_bigop(BigOpKind::Product)),
            T::Eof => Err(ParseError::new(
                tok.span,
                "unexpected end of input, expected an expression",
            )),
            other => Err(ParseError::new(
                tok.span,
                format!("expected an expression, found {other}"),
            )),
        }
    }

    fn parse_ident_primary(&mut self, name: String, span: Span) -> PResult<Expr> {
        let start = span.start;
        if self.ctx.in_integral && differential_var(&name).is_some() {
            return Err(ParseError::new(
                span,
                format!("expected an integrand before `{name}`"),
            ));
        }
        if name == "d"
            && let Some((var, order, len)) = self.derivative_header()?
        {
            for _ in 0..len {
                self.bump();
            }
            let body = self.nested(|p| p.parse_multiplicative())?;
            return Ok(self.node(
                ExprKind::Derivative {
                    var,
                    order,
                    expr: body,
                },
                start,
            ));
        }
        if (name == "sum" || name == "prod")
            && matches!(self.look(1), T::LParen)
            && matches!(self.look(2), T::Ident(_))
            && matches!(self.look(3), T::Assign)
        {
            let op = if name == "sum" {
                BigOpKind::Sum
            } else {
                BigOpKind::Product
            };
            return self.nested(|p| p.parse_bigop(op));
        }
        if name == "int" && matches!(self.look(1), T::LParen) {
            let open = self.skip_from(self.cur() + 1);
            let (one_comma, after) = self.paren_shape(open);
            if one_comma && can_start_operand(&self.tokens[after].kind) {
                return self.nested(|p| p.parse_integral());
            }
        }
        self.bump();
        let ident = self.node(ExprKind::Ident(name.clone()), start);
        if !is_prefix_function(&name) {
            return Ok(ident);
        }
        // `sin^2 x`, `sin^-1 x`
        if self.at(&T::Caret) {
            self.bump();
            let exp = self.parse_exponent()?;
            let callee_name = match &*exp.kind {
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    expr,
                } if TRIG.contains(&name.as_str())
                    && matches!(&*expr.kind, ExprKind::Number { text, imaginary: false } if text == "1") =>
                {
                    Some(format!("arc{name}"))
                }
                _ => None,
            };
            let arg = if self.at(&T::LParen) {
                Some(self.parse_call_args()?)
            } else if self.starts_implicit_operand() {
                Some(vec![self.parse_fn_arg()?])
            } else {
                None
            };
            return Ok(match (arg, callee_name) {
                (None, _) => binary(BinOp::Pow, ident, exp),
                (Some(args), Some(arc)) => {
                    let callee = Expr::new(ExprKind::Ident(arc), span);
                    self.node(ExprKind::Call { callee, args }, start)
                }
                (Some(args), None) => {
                    let call = self.node(
                        ExprKind::Call {
                            callee: ident,
                            args,
                        },
                        start,
                    );
                    binary(BinOp::Pow, call, exp)
                }
            });
        }
        // Paren-less application: `sin x`, `sin 2x`, `sin -x`.
        if !self.at(&T::LParen)
            && (self.starts_implicit_operand() || matches!(self.peek(), T::Minus | T::Plus))
        {
            let arg = self.parse_fn_arg()?;
            return Ok(self.node(
                ExprKind::Call {
                    callee: ident,
                    args: vec![arg],
                },
                start,
            ));
        }
        Ok(ident)
    }

    /// Matches `d/dx` or `d^n/dx^n` at the current position (which holds the
    /// `d`) when an operand follows. Returns `(var, order, token count)`.
    fn derivative_header(&self) -> PResult<Option<(String, u32, usize)>> {
        let diff = |t: &T| match t {
            T::Ident(n) => differential_var(n).map(str::to_string),
            _ => None,
        };
        let int = |t: &T| match t {
            T::Number {
                text,
                imaginary: false,
            } => text.parse::<u32>().ok(),
            _ => None,
        };
        let (var, order, len) = if matches!(self.look(1), T::Slash) {
            match diff(self.look(2)) {
                Some(v) => (v, 1, 3),
                None => return Ok(None),
            }
        } else if matches!(self.look(1), T::Caret)
            && matches!(self.look(3), T::Slash)
            && matches!(self.look(5), T::Caret)
        {
            let (Some(n), Some(var), Some(m)) =
                (int(self.look(2)), diff(self.look(4)), int(self.look(6)))
            else {
                return Ok(None);
            };
            if n != m || n == 0 {
                return Err(ParseError::new(
                    self.peek_span(),
                    format!(
                        "derivative order mismatch: `d^{n}/d{var}^{m}` (orders must match and be positive)"
                    ),
                ));
            }
            (var, n, 7)
        } else {
            return Ok(None);
        };
        Ok(can_start_derivative_body(self.look(len)).then_some((var, order, len)))
    }

    /// Scan from token index `open` (a `(`) to the matching `)`. Returns
    /// whether it holds exactly one top-level comma, and the index of the
    /// first significant token after the `)`.
    fn paren_shape(&self, open: usize) -> (bool, usize) {
        let mut depth = 0usize;
        let mut commas = 0;
        let mut i = open;
        loop {
            match self.tokens[i].kind {
                T::Eof => return (false, i),
                T::LParen | T::LBracket | T::LBrace => depth += 1,
                T::RParen | T::RBracket | T::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return (commas == 1, self.skip_from(i + 1));
                    }
                }
                T::Comma if depth == 1 => commas += 1,
                _ => {}
            }
            i += 1;
        }
    }

    fn parse_integral(&mut self) -> PResult<Expr> {
        let start = self.bump().span.start;
        let bounds = if self.at(&T::LParen) && self.paren_shape(self.cur()).0 {
            self.bump();
            Some(self.with_ctx(GROUP_CTX, |p| -> PResult<_> {
                let lo = p.parse_expr()?;
                p.expect(T::Comma)?;
                let hi = p.parse_expr()?;
                p.expect(T::RParen)?;
                Ok((lo, hi))
            })?)
        } else {
            None
        };
        let ctx = Ctx {
            in_integral: true,
            ..self.ctx
        };
        let body = self.with_ctx(ctx, |p| p.parse_additive())?;
        let var = match self.peek() {
            T::Ident(n) if differential_var(n).is_some() => {
                let v = differential_var(n).unwrap_or_default().to_string();
                self.bump();
                v
            }
            _ => {
                return Err(self.unexpected("differential (e.g. `dx`) closing the integral"));
            }
        };
        Ok(self.node(ExprKind::Integral { bounds, body, var }, start))
    }

    fn parse_bigop(&mut self, op: BigOpKind) -> PResult<Expr> {
        let start = self.bump().span.start;
        self.expect(T::LParen)?;
        let (var, from, to) = self.with_ctx(GROUP_CTX, |p| -> PResult<_> {
            let (var, _) = p.expect_ident()?;
            p.expect(T::Assign)?;
            let from = p.parse_expr()?;
            p.expect(T::Comma)?;
            let to = p.parse_expr()?;
            p.expect(T::RParen)?;
            Ok((var, from, to))
        })?;
        let body = self.parse_multiplicative()?;
        Ok(self.node(
            ExprKind::BigOp {
                op,
                var,
                from,
                to,
                body,
            },
            start,
        ))
    }

    fn parse_piecewise(&mut self, open: Span) -> PResult<Expr> {
        self.bump();
        let start = open.start;
        self.with_ctx(GROUP_CTX, |p| {
            if p.at(&T::RBrace) {
                let close = p.bump();
                return Err(ParseError::new(open.to(close.span), "empty braces `{}`"));
            }
            let mut branches = Vec::new();
            let mut otherwise = None;
            loop {
                let cond = p.parse_expr()?;
                if p.eat(&T::Colon) {
                    let value = p.parse_expr()?;
                    branches.push(Branch {
                        cond,
                        value: Some(value),
                    });
                } else if p.at(&T::RBrace) {
                    if branches.is_empty() {
                        branches.push(Branch { cond, value: None });
                    } else {
                        otherwise = Some(cond);
                    }
                    break;
                } else {
                    return Err(p.unexpected("`:` in piecewise branch"));
                }
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.expect(T::RBrace)?;
            Ok(p.node(
                ExprKind::Piecewise {
                    branches,
                    otherwise,
                },
                start,
            ))
        })
    }
}

fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.to(rhs.span);
    Expr::new(ExprKind::Binary { op, lhs, rhs }, span)
}

fn collapse_add(mut terms: Vec<Term>) -> Expr {
    if terms.len() == 1 {
        return terms.pop().expect("one term").expr;
    }
    let span = terms[0].expr.span.to(terms[terms.len() - 1].expr.span);
    Expr::new(ExprKind::Add(terms), span)
}

fn collapse_mul(mut factors: Vec<Factor>) -> Expr {
    if factors.len() == 1 {
        return factors.pop().expect("one factor").expr;
    }
    let span = factors[0]
        .expr
        .span
        .to(factors[factors.len() - 1].expr.span);
    Expr::new(ExprKind::Mul(factors), span)
}

fn is_callable(e: &Expr) -> bool {
    matches!(
        *e.kind,
        ExprKind::Ident(_) | ExprKind::Prime { .. } | ExprKind::Member { .. }
    )
}

/// Tokens that can begin an operand (used for lookahead decisions).
fn can_start_operand(t: &T) -> bool {
    can_start_derivative_body(t) || matches!(t, T::LBrace | T::Minus | T::Plus)
}

/// Tokens that may follow `d/dx` for it to be a derivative. Signs and `{`
/// are excluded so `d/dx + 1` stays a division and `d/dx {…}` a restriction;
/// write `d/dx (-x)` instead.
fn can_start_derivative_body(t: &T) -> bool {
    matches!(
        t,
        T::Number { .. }
            | T::Ident(_)
            | T::Str(_)
            | T::LParen
            | T::LBracket
            | T::Bar
            | T::Sqrt
            | T::Cbrt
            | T::Integral
            | T::Sum
            | T::Product
    )
}
