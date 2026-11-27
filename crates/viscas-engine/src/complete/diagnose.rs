//! Live diagnostics: parse errors and unknown names, with spans and
//! "did you mean" suggestions, computed without evaluating anything.

use std::collections::HashSet;

use viscas_core::eval::{ALIASES, BUILTINS, CONSTANTS, PLOT_VARS, canonical_name};
use viscas_core::syntax::lexer::lex;
use viscas_core::syntax::parser::{differential_var, is_prefix_function};
use viscas_core::syntax::{
    ExprKind, Keyword, Span, Stmt, StmtKind, TokenKind, parse_program_partial_with,
};

use super::{Scope, candidate_names, suggest};
use crate::engine::Engine;
use crate::help::{ASCII_ALIASES, COMMAND_HELP};
use crate::lines::line_parse_options;

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The input cannot evaluate (parse error, unknown function).
    Error,
    /// Probably a typo (an unknown name close to a known one).
    Warning,
}

/// One problem in an input line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Byte range in the line.
    pub span: Span,
    pub message: String,
    pub severity: Severity,
    /// Replacements for the span, best first (quick fixes).
    pub suggestions: Vec<String>,
}

impl Diagnostic {
    /// `line` with the span replaced by suggestion `index`, and the byte
    /// offset just after the replacement.
    pub fn apply(&self, line: &str, index: usize) -> Option<(String, usize)> {
        let s = self.suggestions.get(index)?;
        let (start, end) = (self.span.start, self.span.end);
        if start > end
            || end > line.len()
            || !line.is_char_boundary(start)
            || !line.is_char_boundary(end)
        {
            return None;
        }
        let out = format!("{}{s}{}", &line[..start], &line[end..]);
        Some((out, start + s.len()))
    }

    /// Whether the span reaches past the end of the input (an incomplete
    /// expression such as `sin(`, still being typed).
    pub fn at_end(&self, line: &str) -> bool {
        self.span.start >= line.trim_end().len()
    }
}

impl Engine {
    /// Problems in `line` (no evaluation): parse errors, calls of unknown
    /// functions, unknown names that look like typos of known ones, and in
    /// the console unknown `:commands` / `:mode` arguments. Sorted by
    /// position.
    pub fn diagnostics(&self, line: &str, scope: Scope) -> Vec<Diagnostic> {
        if line.trim().is_empty() {
            return Vec::new();
        }
        if scope == Scope::Console && line.trim_start().starts_with(':') {
            return command_diagnostics(line);
        }
        let settings = self.settings();
        let opts = line_parse_options(line, settings);
        let (program, errors) = parse_program_partial_with(line, &opts);
        let mut out: Vec<Diagnostic> = errors
            .into_iter()
            .map(|e| Diagnostic {
                span: e.span,
                message: e.message,
                severity: Severity::Error,
                suggestions: Vec::new(),
            })
            .collect();

        let mut known: HashSet<String> = candidate_names(self).into_iter().collect();
        known.extend(["ans", "ans2"].map(String::from));
        for s in &program.stmts {
            bound_names(s, &mut known);
        }
        let (tokens, _) = lex(line);
        let cands = candidate_names(self);
        for (k, t) in tokens.iter().enumerate() {
            let TokenKind::Ident(name) = &t.kind else {
                continue;
            };
            let prev = k.checked_sub(1).map(|p| &tokens[p].kind);
            let next = tokens.get(k + 1).map(|t| &t.kind);
            let binds = matches!(next, Some(TokenKind::Assign | TokenKind::ColonEq))
                || matches!(
                    prev,
                    Some(
                        TokenKind::Dot
                            | TokenKind::Keyword(Keyword::Let | Keyword::For | Keyword::Fn)
                    )
                );
            if binds || is_known(name, &known) {
                continue;
            }
            if out.iter().any(|d| overlaps(d.span, t.span)) {
                continue;
            }
            if matches!(next, Some(TokenKind::LParen)) {
                let mut message = format!("unknown function `{name}`");
                if top_level_equation_head(&program.stmts, t.span) {
                    message.push_str(&format!(" (define it with `{name}(…) :=`)"));
                }
                out.push(Diagnostic {
                    span: t.span,
                    message,
                    severity: Severity::Error,
                    suggestions: suggest(name, &cands),
                });
            } else if name.chars().count() >= 3 {
                let suggestions = suggest(name, &cands);
                if !suggestions.is_empty() {
                    out.push(Diagnostic {
                        span: t.span,
                        message: format!(
                            "unknown name `{name}` (read as a product of its letters)"
                        ),
                        severity: Severity::Warning,
                        suggestions,
                    });
                }
            }
        }
        out.sort_by_key(|d| (d.span.start, d.span.end));
        out
    }
}

fn overlaps(a: Span, b: Span) -> bool {
    a.start < b.end && b.start < a.end
}

fn is_known(name: &str, known: &HashSet<String>) -> bool {
    let canon = canonical_name(name);
    known.contains(name)
        || known.contains(canon)
        || PLOT_VARS.contains(&canon)
        || CONSTANTS.contains(&canon)
        || ASCII_ALIASES.contains(&name)
        || BUILTINS.iter().any(|b| b.name == canon)
        || ALIASES.iter().any(|(a, _)| *a == name)
        || is_prefix_function(name)
        || name.starts_with("log_")
        || differential_var(name).is_some()
}

/// Names a statement binds: definition heads and their parameters,
/// `let`, `for` and `fn` names.
fn bound_names(s: &Stmt, out: &mut HashSet<String>) {
    match &s.kind {
        StmtKind::Definition { lhs, .. } => match &*lhs.kind {
            ExprKind::Ident(n) => {
                out.insert(n.clone());
            }
            ExprKind::Call { callee, args } => {
                if let ExprKind::Ident(n) = &*callee.kind {
                    out.insert(n.clone());
                }
                for a in args {
                    if let ExprKind::Ident(n) = &*a.kind {
                        out.insert(n.clone());
                    }
                }
            }
            _ => {}
        },
        StmtKind::Regression { .. } => {}
        StmtKind::Let { name, .. } | StmtKind::For { var: name, .. } => {
            out.insert(name.clone());
        }
        StmtKind::Fn { name, params, body } => {
            out.insert(name.clone());
            out.extend(params.iter().cloned());
            for s in &body.stmts {
                bound_names(s, out);
            }
        }
        _ => {}
    }
}

/// Whether `span` is the callee of a top-level `f(x) = …` equation (a
/// definition written with `=` while `:=` defines).
fn top_level_equation_head(stmts: &[Stmt], span: Span) -> bool {
    stmts.iter().any(|s| match &s.kind {
        StmtKind::Equation { lhs, .. } => match &*lhs.kind {
            ExprKind::Call { callee, .. } => callee.span == span,
            _ => false,
        },
        _ => false,
    })
}

/// Unknown commands and `:mode` arguments.
fn command_diagnostics(line: &str) -> Vec<Diagnostic> {
    let lead = line.len() - line.trim_start().len();
    let body = &line[lead + 1..];
    let word_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let cmd = &body[..word_end];
    if cmd.is_empty() {
        return Vec::new();
    }
    let span = Span {
        start: lead,
        end: lead + 1 + word_end,
    };
    let names: Vec<String> = COMMAND_HELP.iter().map(|c| c.name.to_string()).collect();
    let full = format!(":{cmd}");
    if !names.contains(&full) {
        return vec![Diagnostic {
            span,
            message: format!("unknown command `{full}`"),
            severity: Severity::Error,
            suggestions: suggest(&full, &names),
        }];
    }
    if cmd == "mode" {
        const MODES: [&str; 6] = ["exact", "approx", "deg", "rad", "real", "complex"];
        let modes: Vec<String> = MODES.iter().map(|m| m.to_string()).collect();
        let mut out = Vec::new();
        let mut pos = lead + 1 + word_end;
        for arg in body[word_end..].split_whitespace() {
            let start = line[pos..].find(arg).map_or(pos, |i| pos + i);
            pos = start + arg.len();
            if !MODES.contains(&arg) {
                out.push(Diagnostic {
                    span: Span { start, end: pos },
                    message: format!("unknown mode `{arg}`"),
                    severity: Severity::Error,
                    suggestions: suggest(arg, &modes),
                });
            }
        }
        return out;
    }
    Vec::new()
}
