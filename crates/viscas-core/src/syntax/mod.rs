//! Lexer, parser, AST and pretty-printer for the Viscas expression and
//! scripting language.

pub mod lexer;
pub mod token;

use std::fmt;

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
