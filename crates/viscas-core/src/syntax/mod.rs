//! Lexer, parser, AST and pretty-printer for the Viscas expression and
//! scripting language.
//!
//! Entry points: [`parse_expr`] (one expression), [`parse_statement`] (one
//! line of the expression list / console), [`parse_program`] (a script) and
//! [`parse_program_partial`] (best-effort parse that keeps the statements
//! that parsed, for editors).
//!
//! # Grammar (loosest binding first)
//!
//! ```text
//! program    := (stmt (NEWLINE | ';'))*
//! stmt       := 'let' IDENT '=' expr
//!             | 'if' head block ('else' (if-stmt | block))?
//!             | 'for' IDENT 'in' head block | 'while' head block
//!             | 'fn' IDENT '(' params ')' block | 'return' expr?
//!             | head ':=' expr restrictions          definition
//!             | expr ('=' expr | '~' expr)? restrictions
//!             | expr '->' expr (',' expr '->' expr)*
//! restrictions := ('{' expr (',' expr)* '}')*
//! block      := '{' program '}'
//! expr       := to                                  (+ nested restriction)
//! to         := or ('to' or)*                        unit conversion
//! or / and   := n-ary chains of 'or' / 'and'
//! not        := 'not' not | compare
//! compare    := additive (cmpop additive)*           chainable: 0 < x < 2
//! additive   := mult (('+' | '-') mult)*             n-ary
//! mult       := unary ('*' unary | '/' juxt | 'mod' unary | <implicit> power)*
//! juxt       := unary (<implicit> power)*            1/2x = 1/(2x)
//! unary      := ('-' | '+') unary | power             -x^2 = -(x^2)
//! power      := postfix ('^' exponent)?               right-assoc
//! exponent   := ('-' | '+') exponent | power          2^-3
//! postfix    := primary (call | index | '.' IDENT | '!' | '%' | "'")*
//! primary    := NUMBER | STRING | IDENT | '(' expr ')' | tuple | list
//!             | range | comprehension | piecewise | '|' expr '|'
//!             | '√' juxt | 'd/dx' mult | '∫' ... 'dx'
//!             | '∑(k=a,b)' mult | '∏(...)' mult
//! ```
//!
//! - `cmpop` is `< <= > >= == !=` (and unicode `≤ ≥ ≠`). A single `=` is an
//!   equality comparison only inside brackets, braces, call arguments and
//!   `if`/`while` heads; at statement level it produces
//!   [`StmtKind::Equation`].
//! - Definitions use `:=` (also `≔`): `a := 3`, `f(x, y) := x y`
//!   ([`StmtKind::Definition`]); the left side must be a name or `f(...)`
//!   with distinct parameter names. With the default
//!   [`DefinitionOp::ColonEq`] a statement-level `=` is *always* an
//!   equation/relation (`x = 3` is a vertical line, `a = 3` an equation in
//!   `a`). [`ParseOptions`] with [`DefinitionOp::Eq`] restores `=`
//!   definitions for `name = ...` / `f(x) = ...` (except `x`, `y`, `r` and
//!   prefix functions). In scripts, `let x = 3` declares a local and
//!   `x = 4` reassigns it; `:=` defines a global.
//! - Inequalities such as `x^2 + y^2 < 4` are plain expression statements.
//! - Restrictions: `{...}` groups at the end of an expression, equation or
//!   regression statement are stored on the statement and apply to the whole
//!   relation (`y = x^2 {0 < x < 2}`, `x^2 + y^2 < 4 {x > 0}`). Inside
//!   brackets/parentheses a trailing `{...}` becomes an
//!   [`ExprKind::Restrict`] node (`[x {x > 0}, -x]`); [`parse_expr`] also
//!   returns one for a trailing group. A `{` never starts an implicit
//!   multiplication; piecewise braces appear only in operand position
//!   (`{x < 0: -x, x}`, `2 + {x > 0}`). Braces never group: `x^{2}` and
//!   `√{x}` are errors (write `x^(2)`).
//! - `name(args)` (also `f'(x)`, `P.f(x)`) is always a call; a later pass may
//!   reinterpret `x(y+1)` as multiplication if `x` is not a function.
//!   `L[i, j, ...]` indexes with one or more indices (`L[2]`, `L[1, 3]`,
//!   range `L[2..4]`, filter `L[L > 3]`); `L[[1, 3]]` is one list index.
//! - Lists `[1, 2]`, ranges `[1..10]` / `[0, 0.1..1]` (`...` also accepted),
//!   comprehensions `[k^2 for k = [1..5], j = L]`, matrices as nested lists.
//! - Absolute value `|x|` becomes `abs(x)`. A `|` in operand position opens
//!   a bar; in operator position it closes the innermost open bar, so
//!   `||x| - 1|` is `abs(abs(x) - 1)` and `|x + |y||` is `abs(x + abs(y))`.
//!   Outside any open bar, a `|` after an operand starts an implicitly
//!   multiplied abs: `|a|b|c|` is `abs(a)·b·abs(c)`.
//! - Calculus: `d/dx f(x)`, `d^2/dx^2 f` (orders must match); `∫(a, b) expr
//!   dx`, indefinite `∫ expr dx` (also `int(a, b) expr dx`); `∑(k = 1, n)
//!   expr` / `sum(k = 1, n) expr`, `∏` / `prod`. The derivative, sum and
//!   product bodies extend over a whole product (`∑(k=1,n) 2k + 1` is
//!   `(∑ 2k) + 1`); the integrand extends to the differential (`dx`, `dθ`,
//!   `dt_1`: the letter `d` followed by one letter and an optional
//!   subscript). `d/dx` is only a derivative when an operand (not a sign or
//!   `{`) follows, so `d/dx + 1` is a division; write `d/dx (-x)`.
//!   `sum(...)`/`int(...)` without the header shape are calls.
//! - `a -> 1, b -> b + 1` is one action statement with several updates.
//! - Limits: nesting deeper than [`parser::MAX_DEPTH`] is an error; sums,
//!   products, `and`/`or` chains, comparison chains and lists are flat, so
//!   long inputs never build deep trees.
//!
//! # Implicit multiplication
//!
//! Juxtaposition multiplies. It is the way formulas are written on paper,
//! so it binds *tighter* than an explicit `/`: the right operand of `/` takes
//! every implicitly multiplied factor that follows, so `1/2x` is `1/(2x)`
//! and `1/2 x` (with a space) is the same — whitespace never changes the
//! meaning. Explicit `*` and `mod` are ordinary left-assoc operators at the
//! same level as `/` (`a / b * c` is `(a/b)·c`). `-2x` is `(-2)·x`.
//! After an operand, the following tokens start an implicit factor:
//!
//! - an identifier (`2x`, `x y`, `2 sin x`). Identifiers may be
//!   multi-letter (`xy` is one name, `km` is a unit); the evaluator splits
//!   an *undefined* multi-letter name into a product of its letters. Single
//!   Greek letters never merge, so `2πr` is `2·π·r`;
//! - a number, unless directly after another number (`x 2` is `x·2`, but
//!   `2 3` is an error, as is `x^2 3`);
//! - `(` (`2(x+1)`, `(a)(b)`), unless the operand is callable (`f(x)`);
//! - `|`, `√`, `∫`, `∑`, `∏`, and `d/dx`;
//! - `[` only right after a number (`2[1, 2]`); otherwise it indexes.
//!
//! Prefix functions (`sin cos tan … ln log exp log_b`, see
//! [`parser::PREFIX_FUNCTIONS`]) and `√` / `∛` can be applied without
//! parentheses. Their argument is an optional sign followed by a
//! juxtaposition of power terms; it stops at any explicit operator and
//! before the next prefix function name. This keeps `sin 2x` reading as
//! the sine of `2x`, while `sin x cos x` stays a product of two functions:
//!
//! | input          | parses as            |
//! |----------------|----------------------|
//! | `sin x`        | `sin(x)`             |
//! | `sin 2x`       | `sin(2·x)`           |
//! | `sin x^2`      | `sin(x^2)`           |
//! | `sin -x`       | `sin(-x)`            |
//! | `sin x cos x`  | `sin(x)·cos(x)`      |
//! | `2 sin x`      | `2·sin(x)`           |
//! | `sin x + 1`    | `sin(x) + 1`         |
//! | `sin x / 2`    | `sin(x) / 2`         |
//! | `sin cos x`    | `sin(cos(x))`        |
//! | `sin(x)^2`, `sin^2 x` | `sin(x)^2`    |
//! | `sin^-1 x`     | `arcsin(x)`          |
//! | `√2x`          | `sqrt(2·x)`          |
//! | `√x + 1`       | `sqrt(x) + 1`        |
//!
//! Units are plain identifiers here: `5 km/h to m/s` is
//! `((5·km)/h) to (m/s)`.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

#[cfg(test)]
mod tests;

use std::fmt;

pub use ast::*;
pub use token::{Keyword, Span, Token, TokenKind};

/// A lexical or syntax error with the offending source range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
}

impl ParseError {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        ParseError {
            span,
            message: message.into(),
        }
    }

    /// 1-based (line, column) of the error start, columns counted in chars.
    pub fn line_col(&self, src: &str) -> (usize, usize) {
        let upto = &src[..self.span.start.min(src.len())];
        let line = upto.matches('\n').count() + 1;
        let col = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        (line, col)
    }

    /// Human-readable report: `line:col: message`, the source line and a
    /// caret marker under the span.
    pub fn render(&self, src: &str) -> String {
        let (line, col) = self.line_col(src);
        let text = src.lines().nth(line - 1).unwrap_or("");
        let line_start = src[..self.span.start.min(src.len())]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let line_end = line_start + text.len();
        let width = src
            .get(self.span.start..self.span.end.min(line_end).max(self.span.start))
            .map_or(1, |s| s.chars().count().max(1));
        format!(
            "{line}:{col}: {}\n{text}\n{}{}",
            self.message,
            " ".repeat(col - 1),
            "^".repeat(width)
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for ParseError {}

fn finish<T>(value: Option<T>, mut errors: Vec<ParseError>) -> Result<T, Vec<ParseError>> {
    errors.sort_by_key(|e| (e.span.start, e.span.end));
    errors.dedup();
    match value {
        Some(v) if errors.is_empty() => Ok(v),
        _ => Err(errors),
    }
}

/// Which operator defines variables and functions at statement level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DefinitionOp {
    /// `a := 3`, `f(x) := x^2` define; `=` is always an equation/relation
    /// (`x = 3` is a vertical line, `a = 3` an equation in `a`). Default.
    #[default]
    ColonEq,
    /// Legacy/alternative: `name = ...` and `f(x, ...) = ...` also define
    /// (unless the name is `x`, `y`, `r`, `θ` or a prefix function such as
    /// `sin`), so `a = 3` defines `a` while `y = x^2` stays a relation.
    /// `:=` still defines.
    Eq,
}

/// Parser configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParseOptions {
    pub definition_op: DefinitionOp,
}

/// Parse a single expression (surrounding newlines allowed). A top-level
/// `=` is an error here; use [`parse_statement`] for definitions/equations.
pub fn parse_expr(src: &str) -> Result<Expr, Vec<ParseError>> {
    let (tokens, mut errors) = lexer::lex(src);
    let mut p = parser::Parser::new(tokens, ParseOptions::default());
    let r = p.expr_entry();
    errors.append(&mut p.errors);
    let value = r.map_err(|e| errors.push(e)).ok();
    finish(value, errors)
}

/// Parse exactly one statement (an expression-list line or console input).
pub fn parse_statement(src: &str) -> Result<Stmt, Vec<ParseError>> {
    parse_statement_with(src, &ParseOptions::default())
}

/// [`parse_statement`] with explicit options.
pub fn parse_statement_with(src: &str, opts: &ParseOptions) -> Result<Stmt, Vec<ParseError>> {
    let (tokens, mut errors) = lexer::lex(src);
    let mut p = parser::Parser::new(tokens, *opts);
    let r = p.statement_entry();
    errors.append(&mut p.errors);
    let value = r.map_err(|e| errors.push(e)).ok();
    finish(value, errors)
}

/// Parse a script. All errors are reported (statement-level recovery).
pub fn parse_program(src: &str) -> Result<Program, Vec<ParseError>> {
    parse_program_with(src, &ParseOptions::default())
}

/// [`parse_program`] with explicit options.
pub fn parse_program_with(src: &str, opts: &ParseOptions) -> Result<Program, Vec<ParseError>> {
    let (program, errors) = parse_program_partial_with(src, opts);
    finish(Some(program), errors)
}

/// Parse a script, returning the statements that parsed successfully along
/// with every error encountered.
pub fn parse_program_partial(src: &str) -> (Program, Vec<ParseError>) {
    parse_program_partial_with(src, &ParseOptions::default())
}

/// [`parse_program_partial`] with explicit options.
pub fn parse_program_partial_with(src: &str, opts: &ParseOptions) -> (Program, Vec<ParseError>) {
    let (tokens, mut errors) = lexer::lex(src);
    let mut p = parser::Parser::new(tokens, *opts);
    let program = p.program();
    errors.append(&mut p.errors);
    errors.sort_by_key(|e| (e.span.start, e.span.end));
    (program, errors)
}
