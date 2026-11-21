//! Tokens produced by the [lexer](super::lexer).

use std::fmt;

/// A half-open byte range `start..end` into the source text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Reserved words. `in` and `d` are *contextual* and lexed as identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Keyword {
    And,
    Or,
    Not,
    Mod,
    To,
    For,
    Let,
    If,
    Else,
    While,
    Fn,
    Return,
}

impl Keyword {
    pub fn lookup(s: &str) -> Option<Keyword> {
        Some(match s {
            "and" => Keyword::And,
            "or" => Keyword::Or,
            "not" => Keyword::Not,
            "mod" => Keyword::Mod,
            "to" => Keyword::To,
            "for" => Keyword::For,
            "let" => Keyword::Let,
            "if" => Keyword::If,
            "else" => Keyword::Else,
            "while" => Keyword::While,
            "fn" => Keyword::Fn,
            "return" => Keyword::Return,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::And => "and",
            Keyword::Or => "or",
            Keyword::Not => "not",
            Keyword::Mod => "mod",
            Keyword::To => "to",
            Keyword::For => "for",
            Keyword::Let => "let",
            Keyword::If => "if",
            Keyword::Else => "else",
            Keyword::While => "while",
            Keyword::Fn => "fn",
            Keyword::Return => "return",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// Raw numeric text (without the imaginary suffix), e.g. `1.5e-3`.
    Number {
        text: String,
        imaginary: bool,
    },
    /// Identifier, subscript normalised to `base_sub` (`y_{12}` → `y_12`).
    Ident(String),
    /// String literal (escapes already processed).
    Str(String),
    Keyword(Keyword),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Bang,
    Percent,
    Prime,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Semi,
    Colon,
    Dot,
    /// `..` or `...`
    DotDot,
    /// `=`
    Assign,
    /// `:=` or `≔` (definition)
    ColonEq,
    /// `==`
    EqEq,
    /// `!=` or `≠`
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Tilde,
    /// `|` (absolute value bars)
    Bar,
    /// `->` or `→`
    Arrow,
    /// `√`
    Sqrt,
    /// `∛`
    Cbrt,
    /// `∫`
    Integral,
    /// `∑`
    Sum,
    /// `∏`
    Product,
    Newline,
    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TokenKind::Number { text, imaginary } => {
                return write!(f, "number `{text}{}`", if *imaginary { "i" } else { "" });
            }
            TokenKind::Ident(name) => return write!(f, "identifier `{name}`"),
            TokenKind::Str(_) => "string literal",
            TokenKind::Keyword(k) => return write!(f, "keyword `{}`", k.as_str()),
            TokenKind::Plus => "`+`",
            TokenKind::Minus => "`-`",
            TokenKind::Star => "`*`",
            TokenKind::Slash => "`/`",
            TokenKind::Caret => "`^`",
            TokenKind::Bang => "`!`",
            TokenKind::Percent => "`%`",
            TokenKind::Prime => "`'`",
            TokenKind::LParen => "`(`",
            TokenKind::RParen => "`)`",
            TokenKind::LBracket => "`[`",
            TokenKind::RBracket => "`]`",
            TokenKind::LBrace => "`{`",
            TokenKind::RBrace => "`}`",
            TokenKind::Comma => "`,`",
            TokenKind::Semi => "`;`",
            TokenKind::Colon => "`:`",
            TokenKind::Dot => "`.`",
            TokenKind::DotDot => "`..`",
            TokenKind::Assign => "`=`",
            TokenKind::ColonEq => "`:=`",
            TokenKind::EqEq => "`==`",
            TokenKind::NotEq => "`!=`",
            TokenKind::Lt => "`<`",
            TokenKind::Le => "`<=`",
            TokenKind::Gt => "`>`",
            TokenKind::Ge => "`>=`",
            TokenKind::Tilde => "`~`",
            TokenKind::Bar => "`|`",
            TokenKind::Arrow => "`->`",
            TokenKind::Sqrt => "`√`",
            TokenKind::Cbrt => "`∛`",
            TokenKind::Integral => "`∫`",
            TokenKind::Sum => "`∑`",
            TokenKind::Product => "`∏`",
            TokenKind::Newline => "end of line",
            TokenKind::Eof => "end of input",
        };
        f.write_str(s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
