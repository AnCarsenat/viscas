//! One rule for reading expression-list / console lines under either
//! definition operator, shared by the console, [`crate::Engine::add_line`]
//! and plot nodes.
//!
//! With [`DefinitionOp::Eq`] the parser turns `name = expr` into a
//! definition whenever `name` is not `x`, `y`, `r` or `θ`. The engine
//! refines that: `name = expr` (or `f(p…) = expr`) only defines when the
//! name is not a plot variable and the right-hand side has no free plot
//! variable (`x y r θ t`, parameters excluded). Otherwise the line is a
//! relation, exactly as with `:=` syntax. `:=` always defines.

use viscas_core::eval::{Env, PLOT_VARS, StatementKind, canonical_name, classify};
use viscas_core::syntax::{
    DefinitionOp, ExprKind, ParseOptions, Stmt, StmtKind, parse_statement_with,
};

use crate::settings::EngineSettings;

/// Parser options to use for `line` under `settings`.
pub fn line_parse_options(line: &str, settings: &EngineSettings) -> ParseOptions {
    let opts = settings.parse_options();
    if opts.definition_op != DefinitionOp::Eq {
        return opts;
    }
    match parse_statement_with(line, &opts) {
        Ok(stmt) if eq_definition_is_relation(&stmt) => ParseOptions::default(),
        _ => opts,
    }
}

/// An `=` definition that the refined rule reads as a relation.
pub fn eq_definition_is_relation(stmt: &Stmt) -> bool {
    let StmtKind::Definition {
        lhs, with_eq: true, ..
    } = &stmt.kind
    else {
        return false;
    };
    let name = match &*lhs.kind {
        ExprKind::Ident(n) => canonical_name(n).to_string(),
        ExprKind::Call { callee, .. } => match &*callee.kind {
            ExprKind::Ident(n) => canonical_name(n).to_string(),
            _ => return false,
        },
        _ => return false,
    };
    if PLOT_VARS.contains(&name.as_str()) {
        return true;
    }
    let info = classify(stmt, &Env::new());
    debug_assert!(matches!(info.kind, StatementKind::Definition { .. }));
    info.free_vars
        .iter()
        .any(|v| PLOT_VARS.contains(&v.as_str()))
}

/// Parse one line with [`line_parse_options`].
pub fn parse_line(
    line: &str,
    settings: &EngineSettings,
) -> Result<Stmt, Vec<viscas_core::syntax::ParseError>> {
    parse_statement_with(line, &line_parse_options(line, settings))
}
