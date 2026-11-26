//! Inline value preview: evaluates a cheap pure console input on a
//! worker thread with a step budget, never touching the engine.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use viscas_core::eval::{classify, eval_expr, format_value};
use viscas_core::syntax::{ExprKind, StmtKind, parse_statement_with};

use crate::engine::Engine;
use crate::eval::env_with;

/// Steps a preview may take before giving up (an input needing more is
/// not "cheap").
pub const PREVIEW_MAX_STEPS: u64 = 20_000;

/// Longest input previewed (bytes).
pub const PREVIEW_MAX_LEN: usize = 200;

/// The evaluation a [`PreviewTask`] runs: exact and decimal texts.
type PreviewFn = Box<dyn FnOnce(Arc<AtomicBool>) -> Option<(String, String)> + Send>;

/// A pending preview ([`Engine::preview_task`]): `Send`, run it on any
/// thread.
pub struct PreviewTask {
    /// The input it evaluates.
    pub input: String,
    run: PreviewFn,
}

impl std::fmt::Debug for PreviewTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewTask")
            .field("input", &self.input)
            .finish_non_exhaustive()
    }
}

impl PreviewTask {
    /// Evaluates; setting `cancel` stops it early. Returns the exact and
    /// decimal texts, `None` on any error, interruption or exhausted
    /// budget.
    pub fn run(self, cancel: Arc<AtomicBool>) -> Option<(String, String)> {
        (self.run)(cancel)
    }
}

impl Engine {
    /// A worker task previewing the value of console input `line`, when
    /// it is a single pure expression: no command, definition, relation,
    /// plot variable or unknown name, and not just a number literal.
    pub fn preview_task(&self, line: &str) -> Option<PreviewTask> {
        let input = line.trim();
        if input.is_empty()
            || input.len() > PREVIEW_MAX_LEN
            || input.starts_with(':')
            || input.contains(['\n', ';'])
        {
            return None;
        }
        let settings = *self.settings();
        let stmt = parse_statement_with(input, &settings.parse_options()).ok()?;
        let StmtKind::Expr { expr, restrictions } = &stmt.kind else {
            return None;
        };
        if !restrictions.is_empty()
            || matches!(&*expr.kind, ExprKind::Number { .. } | ExprKind::Str(_))
        {
            return None;
        }
        let defs = self.evaluator().snapshot_shared();
        let mut env = env_with(&defs);
        for (n, v) in self.console.defs() {
            if !self.evaluator().defines(n) {
                let _ = env.define(n, v.clone());
            }
        }
        let info = classify(&stmt, &env);
        if !info.free_vars.is_empty() {
            return None;
        }
        let expr = expr.clone();
        Some(PreviewTask {
            input: input.to_string(),
            run: Box::new(move |cancel| {
                let mut opts = settings.eval_options(Some(cancel));
                opts.max_steps = PREVIEW_MAX_STEPS;
                let v = eval_expr(&expr, &env, &opts).ok()?;
                Some((
                    format_value(&v, &settings.exact_format()),
                    format_value(&v, &settings.approx_format()),
                ))
            }),
        })
    }
}
