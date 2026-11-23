//! Global environment, name resolution hooks and evaluation options.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::builtins;
use super::value::Value;
use crate::number::Precision;

/// Supplies values for names the [`Env`] does not define (scene nodes).
///
/// Consulted after locals, constants, builtins and globals, before an
/// unknown multi-letter name is split into a product. Must be `Send + Sync`
/// so compiled plot functions can capture it and run on worker threads.
pub trait Resolver: Send + Sync {
    fn resolve(&self, name: &str) -> Option<Value>;
}

impl<F> Resolver for F
where
    F: Fn(&str) -> Option<Value> + Send + Sync,
{
    fn resolve(&self, name: &str) -> Option<Value> {
        self(name)
    }
}

/// Global definitions (variables and functions) plus an optional
/// [`Resolver`] fallback.
#[derive(Clone, Default)]
pub struct Env {
    globals: HashMap<String, Value>,
    resolver: Option<Arc<dyn Resolver>>,
}

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Env")
            .field("globals", &self.globals)
            .field("resolver", &self.resolver.is_some())
            .finish()
    }
}

impl Env {
    pub fn new() -> Self {
        Self::default()
    }

    /// Defines or replaces a global. Reserved names (constants and builtins)
    /// are refused; the error text explains why.
    pub fn define(&mut self, name: &str, value: Value) -> Result<(), String> {
        let name = canonical_name(name);
        if is_reserved(name) {
            return Err(format!(
                "`{name}` is a built-in name and cannot be redefined"
            ));
        }
        if PLOT_VARS.contains(&name) {
            return Err(format!(
                "`{name}` is a plot variable and cannot be defined globally; use another name (e.g. `{name}_0`) or a `let` local"
            ));
        }
        self.globals.insert(name.to_string(), value);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.globals.get(canonical_name(name))
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.globals.remove(canonical_name(name))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.globals.contains_key(canonical_name(name))
    }

    /// Global names in no particular order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.globals.keys().map(String::as_str)
    }

    pub fn set_resolver(&mut self, resolver: Option<Arc<dyn Resolver>>) {
        self.resolver = resolver;
    }

    pub fn resolver(&self) -> Option<&Arc<dyn Resolver>> {
        self.resolver.as_ref()
    }

    /// Existence check without cloning global values (the resolver is
    /// still asked, as it has no separate existence query).
    pub fn has(&self, name: &str) -> bool {
        self.globals.contains_key(name)
            || self
                .resolver
                .as_ref()
                .is_some_and(|r| r.resolve(name).is_some())
    }

    /// Globals first, then the resolver.
    pub fn lookup(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.globals.get(name) {
            return Some(v.clone());
        }
        self.resolver.as_ref().and_then(|r| r.resolve(name))
    }
}

/// ASCII aliases of Greek-letter names: `pi` → `π`, `tau` → `τ`,
/// `phi` → `φ`, `theta` → `θ`, `inf`/`infinity` → `∞`. Every name is
/// canonicalised before it is defined, bound or looked up.
pub fn canonical_name(name: &str) -> &str {
    match name {
        "pi" => "π",
        "tau" => "τ",
        "phi" => "φ",
        "theta" => "θ",
        "inf" | "infinity" => "∞",
        other => other,
    }
}

/// Plot coordinates: always free in statement analysis and never definable
/// as globals (they may be parameters and `let` locals).
pub const PLOT_VARS: &[&str] = &["x", "y", "r", "θ", "t"];

/// Mathematical constants (after [`canonical_name`]).
pub const CONSTANTS: &[&str] = &["π", "τ", "e", "φ", "i", "∞", "true", "false"];

/// Constants and builtin function names cannot be redefined globally (they
/// may be shadowed by parameters and `let` variables).
pub fn is_reserved(name: &str) -> bool {
    let name = canonical_name(name);
    CONSTANTS.contains(&name) || builtins::lookup(name).is_some()
}

/// Angle unit for trigonometric functions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AngleUnit {
    #[default]
    Radians,
    Degrees,
}

/// Evaluation context: numeric mode, precision, angle unit, complex mode,
/// budgets and cancellation.
#[derive(Clone, Debug)]
pub struct EvalOptions {
    /// Convert the final result to decimals (`approx` everything). Exact
    /// arithmetic is still used internally.
    pub approx: bool,
    /// Working precision for inexact operations.
    pub precision: Precision,
    pub angle: AngleUnit,
    /// Complex mode: real inputs may produce complex results (`√-1 = i`).
    /// Off by default: such results are an error ("not real").
    pub complex: bool,
    /// Plot fast path: literals and constants become `f64`, so the whole
    /// evaluation runs on hardware floats.
    pub float64: bool,
    /// Maximum nesting depth (expression nesting + function calls).
    pub max_depth: usize,
    /// Maximum machine stack (bytes) the evaluator may use below the frame
    /// where it was created. Exceeding it is a depth-limit error, never a
    /// stack overflow. The default (1 MiB) suits 2 MiB threads; raise it on
    /// threads with bigger stacks for deeper recursion.
    pub max_stack: usize,
    /// Maximum number of evaluation steps (one per expression node visited;
    /// list operations, matrix products and ranges charge per element).
    pub max_steps: u64,
    /// Maximum number of list elements ranges and comprehensions may create
    /// in one evaluation (memory budget).
    pub max_elements: u64,
    /// Cooperative cancellation; installed for the number tower as well
    /// (see [`crate::number::install_interrupt`]).
    pub interrupt: Option<Arc<AtomicBool>>,
}

impl EvalOptions {
    /// Default nesting limit (the stack budget usually triggers first in
    /// unoptimised builds).
    pub const DEFAULT_MAX_DEPTH: usize = 10_000;
    /// Default stack budget: 1 MiB.
    pub const DEFAULT_MAX_STACK: usize = 1 << 20;
    /// Default step budget.
    pub const DEFAULT_MAX_STEPS: u64 = 10_000_000;
}

impl Default for EvalOptions {
    fn default() -> Self {
        EvalOptions {
            approx: false,
            precision: Precision::DEFAULT,
            angle: AngleUnit::Radians,
            complex: false,
            float64: false,
            max_depth: Self::DEFAULT_MAX_DEPTH,
            max_stack: Self::DEFAULT_MAX_STACK,
            max_elements: 10_000_000,
            max_steps: Self::DEFAULT_MAX_STEPS,
            interrupt: None,
        }
    }
}
