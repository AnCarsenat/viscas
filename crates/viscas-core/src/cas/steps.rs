//! Step-by-step traces (spec §2.1.2).
//!
//! CAS operations call [`record`] with a rule name and the expression
//! before/after the rewrite. Recording is off by default and costs one
//! thread-local check; [`trace`] turns it on for the duration of a
//! closure and returns the steps.

use std::cell::RefCell;

use super::expr::Sym;

/// One rewrite step.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// Human-readable rule (`"power rule"`, `"quadratic formula"`).
    pub rule: String,
    pub before: Sym,
    pub after: Sym,
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} → {}", self.rule, self.before, self.after)
    }
}

/// Maximum number of steps kept per trace.
const MAX_STEPS: usize = 500;

thread_local! {
    static TRACE: RefCell<Option<Vec<Step>>> = const { RefCell::new(None) };
}

/// Is a trace being recorded?
pub fn enabled() -> bool {
    TRACE.with(|t| t.borrow().is_some())
}

/// Records a step if tracing is on. `make` is only called when recording.
pub fn record(rule: &str, make: impl FnOnce() -> (Sym, Sym)) {
    if !enabled() {
        return;
    }
    let (before, after) = make();
    if before == after {
        return;
    }
    TRACE.with(|t| {
        if let Some(v) = t.borrow_mut().as_mut()
            && v.len() < MAX_STEPS
        {
            v.push(Step {
                rule: rule.to_string(),
                before,
                after,
            });
        }
    });
}

/// Runs `f` with tracing on; returns its result and the recorded steps.
/// Nested traces are merged into the outer one.
pub fn trace<T>(f: impl FnOnce() -> T) -> (T, Vec<Step>) {
    let outer = TRACE.with(|t| t.borrow_mut().replace(Vec::new()));
    let r = f();
    let steps = TRACE.with(|t| {
        let mut b = t.borrow_mut();
        let steps = b.take().unwrap_or_default();
        if let Some(mut o) = outer {
            o.extend(steps.iter().cloned());
            *b = Some(o);
        }
        steps
    });
    (r, steps)
}
