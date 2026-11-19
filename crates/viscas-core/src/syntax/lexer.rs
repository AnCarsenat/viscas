//! Hand-written lexer.
//!
//! Lexical rules:
//! - Whitespace other than `\n` is skipped; `\n` produces [`TokenKind::Newline`].
//!   `#` starts a comment running to the end of the line.
//! - Numbers: `12`, `1.5`, `.5`, `1e3`, `1.2E-3`, optional imaginary suffix
//!   `i` (`3i`, `2.5i`) when the `i` is not followed by an identifier
//!   character. A `.` is only part of a number if a digit follows it, so
//!   `1..10` lexes as `1`, `..`, `10`. The raw text is kept verbatim.
//! - Identifiers: a Latin/other letter followed by letters and digits
//!   (`x`, `x1`, `km`, `sin`), **or** a single Greek letter (`π`, `θ`) —
//!   Greek letters never merge with neighbours, so `2πr` is `2 π r`.
//!   An optional subscript follows: `x_1`, `x_ab`, `y_{12}`; the identifier
//!   name is normalised to `base_sub` (so `y_{12}` and `y_12` are the same
//!   name). `∞` is a one-character identifier. `d` directly followed by a
//!   Greek letter (`dθ`) is one identifier so differentials work.
//! - Unicode operator aliases: `· × ⋅` → `*`, `÷` → `/`, `−` → `-`,
//!   `≤ ≥ ≠`, `→` → `->`, `′` → `'`, `**` → `^`, `...` → `..`.

use super::ParseError;
use super::token::{Keyword, Span, Token, TokenKind};

/// Lex `src` into tokens (always terminated by [`TokenKind::Eof`]) plus any
/// lexical errors. Offending characters are skipped so parsing can continue.
pub fn lex(src: &str) -> (Vec<Token>, Vec<ParseError>) {
    let mut lx = Lexer {
        src,
        pos: 0,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    lx.run();
    (lx.tokens, lx.errors)
}

fn is_greek(c: char) -> bool {
    matches!(c, '\u{0370}'..='\u{03FF}' | '\u{1F00}'..='\u{1FFF}')
}

/// Characters that continue a (non-Greek) identifier.
fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() && !is_greek(c)
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic()
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<ParseError>,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
    }

    fn error(&mut self, start: usize, message: impl Into<String>) {
        self.errors
            .push(ParseError::new(Span::new(start, self.pos), message));
    }

    fn run(&mut self) {
        while let Some(c) = self.peek() {
            let start = self.pos;
            if c == '\n' {
                self.bump();
                self.push(TokenKind::Newline, start);
            } else if c.is_whitespace() {
                self.bump();
            } else if c == '#' {
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.bump();
                }
            } else if c.is_ascii_digit()
                || (c == '.' && self.peek_nth(1).is_some_and(|d| d.is_ascii_digit()))
            {
                self.number(start);
            } else if c == '∞' {
                self.bump();
                self.push(TokenKind::Ident("∞".into()), start);
            } else if is_greek(c) {
                self.bump();
                let mut name = c.to_string();
                self.subscript(&mut name);
                self.push(TokenKind::Ident(name), start);
            } else if is_ident_start(c) {
                self.ident(start);
            } else if c == '"' {
                self.string(start);
            } else {
                self.punct(c, start);
            }
        }
        let end = self.src.len();
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(end, end),
        });
    }

    fn eat_digits(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
    }

    fn number(&mut self, start: usize) {
        self.eat_digits();
        if self.peek() == Some('.') && self.peek_nth(1).is_some_and(|d| d.is_ascii_digit()) {
            self.bump();
            self.eat_digits();
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let exp_follows = match self.peek_nth(1) {
                Some(d) if d.is_ascii_digit() => true,
                Some('+' | '-') => self.peek_nth(2).is_some_and(|d| d.is_ascii_digit()),
                _ => false,
            };
            if exp_follows {
                self.bump();
                if matches!(self.peek(), Some('+' | '-')) {
                    self.bump();
                }
                self.eat_digits();
            }
        }
        let text = self.src[start..self.pos].to_string();
        let imaginary = self.peek() == Some('i')
            && !self
                .peek_nth(1)
                .is_some_and(|c| is_ident_continue(c) || c == '_');
        if imaginary {
            self.bump();
        }
        self.push(TokenKind::Number { text, imaginary }, start);
    }

    fn ident(&mut self, start: usize) {
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        let mut name = self.src[start..self.pos].to_string();
        // Differentials of Greek variables (`dθ`) form one identifier.
        if name == "d"
            && let Some(c) = self.peek().filter(|&c| is_greek(c))
        {
            self.bump();
            name.push(c);
        }
        if !self.subscript(&mut name)
            && let Some(kw) = Keyword::lookup(&name)
        {
            self.push(TokenKind::Keyword(kw), start);
            return;
        }
        self.push(TokenKind::Ident(name), start);
    }

    /// Lex an optional `_sub` / `_{sub}` and append it to `name`.
    /// Returns whether a subscript was present.
    fn subscript(&mut self, name: &mut String) -> bool {
        if self.peek() != Some('_') {
            return false;
        }
        let start = self.pos;
        self.bump();
        let mut sub = String::new();
        if self.peek() == Some('{') {
            self.bump();
            loop {
                match self.peek() {
                    Some('}') => {
                        self.bump();
                        break;
                    }
                    Some(c) if c.is_alphanumeric() || c == '_' => {
                        sub.push(c);
                        self.bump();
                    }
                    _ => {
                        self.error(start, "unterminated subscript, expected `}`");
                        break;
                    }
                }
            }
        } else {
            while let Some(c) = self.peek() {
                if c.is_alphanumeric() || c == '_' {
                    sub.push(c);
                    self.bump();
                } else {
                    break;
                }
            }
        }
        if sub.is_empty() {
            self.error(start, "expected subscript after `_`");
        } else {
            name.push('_');
            name.push_str(&sub);
        }
        true
    }

    fn string(&mut self, start: usize) {
        self.bump();
        let mut value = String::new();
        loop {
            match self.bump() {
                None | Some('\n') => {
                    self.error(start, "unterminated string literal");
                    break;
                }
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    other => {
                        let esc_start = self.pos - other.map_or(0, char::len_utf8) - 1;
                        self.error(esc_start, "unknown escape sequence");
                    }
                },
                Some(c) => value.push(c),
            }
        }
        self.push(TokenKind::Str(value), start);
    }

    fn punct(&mut self, c: char, start: usize) {
        use TokenKind as T;
        self.bump();
        let next = self.peek();
        let kind = match c {
            '+' => T::Plus,
            '-' if next == Some('>') => {
                self.bump();
                T::Arrow
            }
            '-' | '−' => T::Minus,
            '*' if next == Some('*') => {
                self.bump();
                T::Caret
            }
            '*' | '·' | '×' | '⋅' => T::Star,
            '/' | '÷' => T::Slash,
            '^' => T::Caret,
            '!' if next == Some('=') => {
                self.bump();
                T::NotEq
            }
            '!' => T::Bang,
            '%' => T::Percent,
            '\'' | '′' => T::Prime,
            '(' => T::LParen,
            ')' => T::RParen,
            '[' => T::LBracket,
            ']' => T::RBracket,
            '{' => T::LBrace,
            '}' => T::RBrace,
            ',' => T::Comma,
            ';' => T::Semi,
            ':' => T::Colon,
            '.' if next == Some('.') => {
                self.bump();
                if self.peek() == Some('.') {
                    self.bump();
                }
                T::DotDot
            }
            '.' => T::Dot,
            '=' if next == Some('=') => {
                self.bump();
                T::EqEq
            }
            '=' => T::Assign,
            '≠' => T::NotEq,
            '<' if next == Some('=') => {
                self.bump();
                T::Le
            }
            '<' => T::Lt,
            '≤' => T::Le,
            '>' if next == Some('=') => {
                self.bump();
                T::Ge
            }
            '>' => T::Gt,
            '≥' => T::Ge,
            '~' => T::Tilde,
            '→' => T::Arrow,
            '√' => T::Sqrt,
            '∛' => T::Cbrt,
            '∫' => T::Integral,
            '∑' => T::Sum,
            '∏' => T::Product,
            _ => {
                self.error(start, format!("unexpected character `{c}`"));
                return;
            }
        };
        self.push(kind, start);
    }
}
