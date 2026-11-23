//! Identifier rules, built-in names, and classification of expression-list
//! lines into node contents.

use std::collections::HashSet;
use std::sync::OnceLock;

use viscas_core::syntax::{
    self, ExprKind, ParseError, StmtKind, TokenKind, lexer, parser::PREFIX_FUNCTIONS,
};

use crate::node::{NodeContent, NodeKind};

/// Is `s` a canonical identifier of the expression language (one
/// identifier token, not a keyword, subscripts in `base_sub` form)?
pub fn is_valid_identifier(s: &str) -> bool {
    let (tokens, errors) = lexer::lex(s);
    if !errors.is_empty() {
        return false;
    }
    match tokens.as_slice() {
        [first, last] => {
            matches!(&first.kind, TokenKind::Ident(n) if n == s)
                && first.span.start == 0
                && first.span.end == s.len()
                && matches!(last.kind, TokenKind::Eof)
        }
        _ => false,
    }
}

/// Names that resolve without a defining node (functions, constants,
/// units). The scene uses it to tell unknown references from library
/// names; the evaluator crate provides the authoritative implementation.
pub trait Builtins: Send + Sync {
    fn is_builtin(&self, name: &str) -> bool;
}

impl<F: Fn(&str) -> bool + Send + Sync> Builtins for F {
    fn is_builtin(&self, name: &str) -> bool {
        self(name)
    }
}

/// A conservative default list: prefix functions, common functions and
/// constants. Units are not included.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultBuiltins;

const EXTRA_BUILTINS: &[&str] = &[
    // constants
    "pi",
    "π",
    "e",
    "i",
    "tau",
    "τ",
    "phi",
    "φ",
    "∞",
    "inf",
    "infinity",
    "true",
    "false",
    "ans",
    // elementary
    "sqrt",
    "cbrt",
    "nthroot",
    "abs",
    "sign",
    "floor",
    "ceil",
    "round",
    "min",
    "max",
    "gcd",
    "lcm",
    "mod",
    "nCr",
    "nPr",
    "factorial",
    "re",
    "im",
    "conj",
    "arg",
    // lists and statistics
    "total",
    "sum",
    "prod",
    "length",
    "count",
    "mean",
    "median",
    "mode",
    "quantile",
    "quartile",
    "stdev",
    "stdevp",
    "var",
    "varp",
    "mad",
    "cov",
    "corr",
    "spearman",
    "stats",
    "sort",
    "shuffle",
    "unique",
    "join",
    "repeat",
    "random",
    // distributions
    "normaldist",
    "tdist",
    "poissondist",
    "binomialdist",
    "uniformdist",
    "pdf",
    "cdf",
    // geometry and colors
    "point",
    "line",
    "segment",
    "ray",
    "vector",
    "circle",
    "arc",
    "polygon",
    "distance",
    "midpoint",
    "intersection",
    "perpendicular",
    "parallel",
    "angle",
    "translate",
    "rotate",
    "reflect",
    "dilate",
    "rgb",
    "hsv",
    // CAS
    "simplify",
    "expand",
    "factor",
    "collect",
    "partialfraction",
    "solve",
    "nsolve",
    "roots",
    "derivative",
    "integrate",
    "int",
    "limit",
    "series",
    "taylor",
    "det",
    "inv",
    "transpose",
    "rank",
    "eigen",
    "rref",
    "isprime",
    "factorint",
    "eval",
    "print",
];

fn default_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        PREFIX_FUNCTIONS
            .iter()
            .chain(EXTRA_BUILTINS)
            .copied()
            .collect()
    })
}

impl Builtins for DefaultBuiltins {
    fn is_builtin(&self, name: &str) -> bool {
        default_set().contains(name) || (name.starts_with("log_") && name.len() > 4)
    }
}

/// A fresh identifier derived from `base` for which `taken` is false:
/// `base` itself if free, else `base_2`, `base_3`, ... (`x_12` → `x_122`).
pub fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = if is_valid_identifier(base) { base } else { "n" };
    if !taken(base) {
        return base.to_string();
    }
    for n in 2.. {
        let candidate = if base.contains('_') {
            format!("{base}{n}")
        } else {
            format!("{base}_{n}")
        };
        if is_valid_identifier(&candidate) && !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Which operator introduces a definition in expression-list lines — the
/// core parser's setting: [`DefinitionOp::ColonEq`] (default: `a := 3`,
/// `f(x) := x^2`; a plain `=` is always a relation) or
/// [`DefinitionOp::Eq`] (`a = 3` defines, `y = …` plots).
pub use viscas_core::syntax::DefinitionOp;

/// Options for [`parse_definition_with`] / [`definition_line_with`]: the
/// core parser options.
pub type LineOptions = viscas_core::syntax::ParseOptions;

/// [`parse_definition_with`] using the default options.
pub fn parse_definition(line: &str) -> Result<NodeContent, Vec<ParseError>> {
    parse_definition_with(line, LineOptions::default())
}

/// Classify one expression-list / console line into node content:
///
/// - `a := 3` → Variable `a`; `P := (1, 2)` → Point; `L := [1..5]` → List
/// - `f(x, y) := body` → Function
/// - every `=` line (`y = …`, `x = …`, `r = …`, implicit equations),
///   inequalities and tuples → Plot
/// - `lhs ~ rhs` → Regression (no parameters; the caller fills them in)
/// - `a -> …` → Action
/// - any other expression → unnamed Variable (anonymous expression)
///
/// With [`DefinitionOp::Eq`] the parser also turns `name = …` /
/// `f(x) = …` into definitions (except for `x`, `y`, `r`, `θ` and prefix
/// functions such as `sin`).
pub fn parse_definition_with(
    line: &str,
    options: LineOptions,
) -> Result<NodeContent, Vec<ParseError>> {
    let stmt = syntax::parse_statement_with(line, &options)?;
    let text = line.trim().to_string();
    Ok(match &stmt.kind {
        StmtKind::Equation { .. } => NodeContent::plot(text),
        StmtKind::Definition { lhs, rhs, .. } => {
            let body = line[rhs.span.start..stmt.span.end].trim().to_string();
            match &*lhs.kind {
                ExprKind::Ident(name) => match &*rhs.kind {
                    ExprKind::Tuple(items) if items.len() == 2 => NodeContent::point(name, body),
                    ExprKind::List(_) | ExprKind::Range { .. } | ExprKind::Comprehension { .. } => {
                        NodeContent::list(name, body)
                    }
                    _ => NodeContent::variable(name, body),
                },
                ExprKind::Call { callee, args } => {
                    let params: Option<Vec<&str>> = args
                        .iter()
                        .map(|a| match &*a.kind {
                            ExprKind::Ident(p) => Some(p.as_str()),
                            _ => None,
                        })
                        .collect();
                    match (&*callee.kind, params) {
                        (ExprKind::Ident(f), Some(params)) => {
                            NodeContent::function(f, &params, body)
                        }
                        _ => NodeContent::plot(text),
                    }
                }
                _ => NodeContent::plot(text),
            }
        }
        StmtKind::Regression { .. } => NodeContent::regression(text, &[]),
        StmtKind::Action { .. } => NodeContent::action(text),
        StmtKind::Expr { expr, restrictions } => match &*expr.kind {
            ExprKind::Compare { .. } | ExprKind::Tuple(_) => NodeContent::plot(text),
            _ if !restrictions.is_empty() => NodeContent::plot(text),
            _ => NodeContent::expression(text),
        },
        _ => NodeContent::script(crate::node::ScriptLanguage::Viscas, text),
    })
}

/// [`definition_line_with`] using the default options.
pub fn definition_line(content: &NodeContent) -> Option<String> {
    definition_line_with(content, LineOptions::default())
}

/// Render a node back to an expression-list line (`f(x) := x^2`), the
/// inverse of [`parse_definition_with`] for expression-like kinds.
pub fn definition_line_with(content: &NodeContent, options: LineOptions) -> Option<String> {
    let name = content.name.as_deref();
    let op = match options.definition_op {
        DefinitionOp::Eq => "=",
        DefinitionOp::ColonEq => ":=",
    };
    Some(match (&content.kind, name) {
        (NodeKind::Variable { expr }, Some(n))
        | (NodeKind::Point { expr }, Some(n))
        | (NodeKind::List { expr }, Some(n)) => format!("{n} {op} {expr}"),
        (NodeKind::Slider { value, .. }, Some(n)) => format!("{n} {op} {value}"),
        (NodeKind::Variable { expr }, None)
        | (NodeKind::Point { expr }, None)
        | (NodeKind::List { expr }, None) => expr.clone(),
        (NodeKind::Function { params, body }, Some(n)) => {
            format!("{n}({}) {op} {body}", params.join(", "))
        }
        (NodeKind::Plot { relation }, _) => relation.clone(),
        (NodeKind::Action { source, .. }, _) | (NodeKind::Regression { source, .. }, _) => {
            source.clone()
        }
        _ => return None,
    })
}
