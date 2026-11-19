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
//!             | expr (('=' | '~' | '->') expr)?
//! block      := '{' program '}'
//! expr       := to ('{' conds '}')*                  restriction
//! to         := or ('to' or)*                        unit conversion
//! or / and   := left-assoc chains of 'or' / 'and'
//! not        := 'not' not | compare
//! compare    := additive (cmpop additive)*           chainable: 0 < x < 2
//! additive   := mult (('+' | '-') mult)*
//! mult       := unary (('*' | '/' | 'mod') unary | <implicit> power)*
//! unary      := ('-' | '+') unary | power             -x^2 = -(x^2)
//! power      := postfix ('^' exponent)?               right-assoc
//! exponent   := ('-' | '+') exponent | power          2^-3
//! postfix    := primary (call | index | '.' IDENT | '!' | '%' | "'")*
//! primary    := NUMBER | STRING | IDENT | '(' expr ')' | tuple | list
//!             | range | comprehension | piecewise | '√' exponent
//!             | 'd/dx' mult | '∫' ... 'dx' | '∑(k=a,b)' mult | '∏(...)' mult
//! ```
//!
//! - `cmpop` is `< <= > >= == !=` (and unicode `≤ ≥ ≠`). A single `=` is an
//!   equality comparison only inside brackets, braces, call arguments and
//!   `if`/`while` heads; at statement level it produces
//!   [`StmtKind::Equation`] (definition vs. equation is decided later).
//! - Inequalities such as `x^2 + y^2 < 4` are plain expression statements.
//! - A `{...}` directly following an expression is a *restriction*
//!   (`y = x^2 {0 < x < 2}` restricts the right-hand side). A `{` never
//!   starts an implicit multiplication; piecewise braces appear only in
//!   operand position (`{x < 0: -x, x}`, `2 + {x > 0}`).
//! - `name(args)` (also `f'(x)`, `P.f(x)`) is always a call; a later pass may
//!   reinterpret `x(y+1)` as multiplication if `x` is not a function.
//!   `L[...]` is indexing (list `L[1, 3]`, range `L[2..4]`, filter `L[L > 3]`).
//! - Lists `[1, 2]`, ranges `[1..10]` / `[0, 0.1..1]` (`...` also accepted),
//!   comprehensions `[k^2 for k = [1..5], j = L]`, matrices as nested lists.
//! - Calculus: `d/dx f(x)`, `d^2/dx^2 f`; `∫(a, b) expr dx`, indefinite
//!   `∫ expr dx` (also `int(a, b) expr dx`); `∑(k = 1, n) expr` /
//!   `sum(k = 1, n) expr`, `∏` / `prod`. The derivative, sum and product
//!   bodies extend over a whole product (`∑(k=1,n) 2k + 1` is
//!   `(∑ 2k) + 1`); the integrand extends to the differential (`dx`, `dθ`,
//!   `dt_1`: the letter `d` followed by one letter and an optional
//!   subscript). `sum(...)`/`int(...)` without that header shape are calls.
//! - `√x` / `∛x` become calls to `sqrt` / `cbrt`.
//!
//! # Implicit multiplication (modelled on Desmos)
//!
//! Juxtaposition multiplies with the same precedence as `*`, left-assoc,
//! so `1/2x` is `(1/2)·x` and `-2x` is `(-2)·x`. After an operand, the
//! following tokens start an implicit factor:
//!
//! - an identifier (`2x`, `x y`, `2 sin x`) — note identifiers may be
//!   multi-letter (`xy` is one name, `km` is a unit); single Greek letters
//!   never merge, so `2πr` is `2·π·r`;
//! - a number, unless directly after another number (`x 2` is `x·2`, but
//!   `2 3` is an error);
//! - `(` (`2(x+1)`, `(a)(b)`), unless the operand is callable (`f(x)`);
//! - `√`, `∫`, `∑`, `∏`, and `d/dx`;
//! - `[` only right after a number (`2[1, 2]`); otherwise it indexes.
//!
//! Prefix functions (`sin cos tan … ln log exp log_b`, see
//! [`parser::PREFIX_FUNCTIONS`]) can be applied without parentheses. Their
//! argument is a juxtaposition of power terms, stopping at any explicit
//! operator and before the next prefix function name:
//!
//! | input          | parses as            |
//! |----------------|----------------------|
//! | `sin x`        | `sin(x)`             |
//! | `sin 2x`       | `sin(2·x)`           |
//! | `sin x^2`      | `sin(x^2)`           |
//! | `sin x cos x`  | `sin(x)·cos(x)`      |
//! | `2 sin x`      | `2·sin(x)`           |
//! | `sin x + 1`    | `sin(x) + 1`         |
//! | `sin x / 2`    | `sin(x) / 2`         |
//! | `sin cos x`    | `sin(cos(x))`        |
//! | `sin(x)^2`, `sin^2 x` | `sin(x)^2`    |
//! | `sin^-1 x`     | `arcsin(x)`          |
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

/// Parse a single expression (surrounding newlines allowed). A top-level
/// `=` is an error here; use [`parse_statement`] for definitions/equations.
pub fn parse_expr(src: &str) -> Result<Expr, Vec<ParseError>> {
    let (tokens, mut errors) = lexer::lex(src);
    let mut p = parser::Parser::new(tokens);
    let r = p.expr_entry();
    errors.append(&mut p.errors);
    let value = r.map_err(|e| errors.push(e)).ok();
    finish(value, errors)
}

/// Parse exactly one statement (an expression-list line or console input).
pub fn parse_statement(src: &str) -> Result<Stmt, Vec<ParseError>> {
    let (tokens, mut errors) = lexer::lex(src);
    let mut p = parser::Parser::new(tokens);
    let r = p.statement_entry();
    errors.append(&mut p.errors);
    let value = r.map_err(|e| errors.push(e)).ok();
    finish(value, errors)
}

/// Parse a script. All errors are reported (statement-level recovery).
pub fn parse_program(src: &str) -> Result<Program, Vec<ParseError>> {
    let (program, errors) = parse_program_partial(src);
    finish(Some(program), errors)
}

/// Parse a script, returning the statements that parsed successfully along
/// with every error encountered.
pub fn parse_program_partial(src: &str) -> (Program, Vec<ParseError>) {
    let (tokens, mut errors) = lexer::lex(src);
    let mut p = parser::Parser::new(tokens);
    let program = p.program();
    errors.append(&mut p.errors);
    errors.sort_by_key(|e| (e.span.start, e.span.end));
    (program, errors)
}
