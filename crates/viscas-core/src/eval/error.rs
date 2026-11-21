//! Evaluation errors.

use std::fmt;

use crate::number::NumberError;
use crate::syntax::{ParseError, Span};

/// What went wrong, for programmatic matching. The human-readable text is
/// in [`EvalError::message`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// A numeric operation failed (division by zero, domain, ...).
    Number(NumberError),
    /// A name that is neither local, global, constant, builtin, supplied by
    /// the resolver, nor splittable into known names.
    UnknownName(String),
    /// Operand of the wrong type (`[1, 2] + true`, calling a number, ...).
    Type,
    /// Wrong number of arguments.
    Arity,
    /// Index out of range or not an integer.
    Index,
    /// The result is not real and complex mode is off.
    NotReal,
    /// The step budget was exhausted (runaway loop).
    StepLimit,
    /// The nesting / recursion depth limit was exceeded.
    DepthLimit,
    /// Cancelled through the interrupt flag.
    Interrupted,
    /// A feature that is planned but not implemented yet (units, CAS, ...).
    Unsupported,
    /// Definition of a reserved name, assignment to a non-variable, ...
    Definition,
    /// The source text did not parse.
    Parse,
    /// Anything else (bad range step, malformed matrix, ...).
    Other,
}

/// An evaluation error with the source range it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalError {
    pub kind: ErrorKind,
    pub message: String,
    pub span: Span,
}

/// Result alias for evaluation.
pub type EvalResult<T> = Result<T, EvalError>;

impl EvalError {
    pub fn new(kind: ErrorKind, message: impl Into<String>, span: Span) -> Self {
        EvalError {
            kind,
            message: message.into(),
            span,
        }
    }

    pub fn ty(message: impl Into<String>, span: Span) -> Self {
        Self::new(ErrorKind::Type, message, span)
    }

    pub fn other(message: impl Into<String>, span: Span) -> Self {
        Self::new(ErrorKind::Other, message, span)
    }

    pub fn unsupported(message: impl Into<String>, span: Span) -> Self {
        Self::new(ErrorKind::Unsupported, message, span)
    }

    pub fn number(err: NumberError, span: Span) -> Self {
        let kind = match err {
            NumberError::Interrupted => ErrorKind::Interrupted,
            _ => ErrorKind::Number(err.clone()),
        };
        Self::new(kind, err.to_string(), span)
    }

    /// Converts the first parse error (if any).
    pub fn from_parse(errors: &[ParseError]) -> Self {
        match errors.first() {
            Some(e) => Self::new(ErrorKind::Parse, e.message.clone(), e.span),
            None => Self::new(ErrorKind::Parse, "parse error", Span::default()),
        }
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for EvalError {}

/// Attach a span to a numeric result.
pub(crate) trait NumSpan<T> {
    fn at(self, span: Span) -> EvalResult<T>;
}

impl<T> NumSpan<T> for Result<T, NumberError> {
    fn at(self, span: Span) -> EvalResult<T> {
        self.map_err(|e| EvalError::number(e, span))
    }
}
