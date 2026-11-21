//! Evaluator: values, environments, expression evaluation, scripting,
//! statement analysis, the `f64` plot fast path and the core builtin
//! library.
//!
//! # Entry points
//!
//! - [`Session`]: an [`Env`] plus [`EvalOptions`]; `session.eval("f(2)")`.
//! - [`eval_expr`] (read-only), [`exec_statement`] (top-level line:
//!   definitions, relations, values), [`run_program`] (script).
//! - [`classify`]: what a statement means ([`StatementKind`]) and which
//!   names it uses, for the plot layer and the scene.
//! - [`compile_plot_fn`]: a `Send + Sync` real function of the plot
//!   variables, allocation-free for ordinary formulas.
//! - [`format_value`]: exact (`1/3`) or decimal (`0.333333333333`) text.
//!
//! # Values
//!
//! [`Value`] is a number of the [numeric tower](crate::number), a boolean,
//! a string, a list, a point/vector (`Tuple`, any dimension), a numeric
//! [`Matrix`], a function (user closure or builtin) or `Undefined`. The
//! enum is non-exhaustive: the CAS adds a symbolic variant later.
//!
//! - A list literal whose elements are equal-length lists of numbers is a
//!   matrix (`[[1, 2], [3, 4]]`); `*` between matrices is the matrix
//!   product, matrix × list is matrix × vector, `M^n` (n ≥ 0) repeated
//!   product, `M.T` / `transpose(M)` the transpose.
//! - `Undefined` comes from failed restrictions and piecewise expressions
//!   without a matching branch; it propagates through arithmetic and counts
//!   as false in conditions. Numeric failures (division by zero, domain
//!   errors) are *errors* with a span; the plot path turns them into NaN.
//!
//! # Evaluation rules
//!
//! - **Exact first.** Literals are exact (`0.1` = 1/10); arithmetic stays
//!   exact while possible (`1/3 + 1/6 = 1/2`, `8^(1/3) = 2`). Irrational
//!   results are floats at [`EvalOptions::precision`]. In approx mode the
//!   final value is converted to decimals. In `float64` mode (plots)
//!   literals and constants are `f64`.
//! - **Real vs complex.** Off by default: a real input producing a complex
//!   result (`√-1`, `ln(-1)`) is a [`ErrorKind::NotReal`] error, and a
//!   negative base with a fractional exponent uses real odd roots
//!   (`(-8)^(1/3) = -2`, also `(-8)^(2/3) = 4`; even roots are an error).
//!   An explicit `i` is always allowed. Complex mode uses principal
//!   branches.
//! - **Angles.** In degree mode trig functions take degrees (with exact
//!   rational values at multiples of 30°/45°, e.g. `sin 30 = 1/2`) and
//!   inverse trig functions, `atan2` and `arg` return degrees.
//! - **Broadcasting.** Arithmetic, comparisons, `and`/`or`/`not` and
//!   numeric builtins apply element-wise to lists (`[1, 2] + 1`,
//!   `[1, 2] * [3, 4]`, `sin([0, 1])`); lists must have equal lengths.
//!   Points add/subtract component-wise and scale by numbers. A piecewise
//!   or restriction with a list condition selects element-wise. User
//!   functions are not mapped explicitly, but their bodies broadcast.
//! - **Comparisons** chain (`0 < x < 2` = `0 < x and x < 2`). `==` on
//!   numbers is numeric (`1 == 1.0`); other values compare structurally.
//! - **Indexing** is 1-based: `L[2]`, `L[[1, 3]]` (selection), `L[2..4]`,
//!   `L[L > 3]` (boolean filter), `M[i, j]` = `M[i][j]`; out of range is an
//!   [`ErrorKind::Index`] error.
//! - **Members**: points `.x .y .z .w`, numbers `.re .im`, matrices
//!   `.T .rows .cols`; on a list the member maps over the elements
//!   (`L.x` = x-coordinates). Lists have no `.length` (use `len(L)`).
//!   `v.f(args)` calls `f(v, args)` when `f` is not a member
//!   (`L.sort()`, `L.sum()`).
//! - **Calculus.** `∑`/`∏` with integer bounds are exact loops. `d/dx`,
//!   `f'(x)` and definite `∫` are numeric (`f64` results): 5-point central
//!   differences, adaptive Gauss–Kronrod 7/15 (infinite bounds by
//!   substitution). Indefinite integrals, symbolic derivatives and
//!   infinite sums need the CAS (marked hooks in `calculus.rs`).
//! - **Units** (`5 km/h to m/s`) report "units are not supported yet".
//!
//! # Name resolution
//!
//! Names are canonicalised (`pi` → `π`, `tau` → `τ`, `phi` → `φ`,
//! `theta` → `θ`, `inf` → `∞`) and resolved in this order:
//!
//! 1. local variables (parameters, `let`, loop and comprehension
//!    variables, captured closure variables);
//! 2. constants `π τ e φ i` (`∞` only as an integral bound);
//! 3. builtins (table below);
//! 4. globals of the [`Env`], then the [`Resolver`] (scene nodes);
//! 5. an unknown multi-letter name is split into a product of known pieces
//!    (`xy` = `x·y`, `2ab` = `2·a·b`; longest known prefix first, digits
//!    and subscripts stay attached: `ab2` = `a·b2`). A function piece
//!    applies to the rest (`sinx` = `sin(x)` when `x` is known). If some
//!    piece is unknown the error is "unknown name `xy`" with the span.
//!
//! Constants and builtins cannot be redefined globally (they can be
//! shadowed by parameters and `let`).
//!
//! **Call vs multiply.** `name(args)` calls `name` if it resolves to a
//! function; otherwise it multiplies (`a(b + 1)` = `a·(b + 1)`,
//! `a(1, 2)` = `a·(1, 2)`). For an unknown split name the last piece is
//! applied (`af(x)` = `a·f(x)`). `log_2(x)` / `log_b x` use the subscript
//! as the base.
//!
//! # Statements
//!
//! At top level ([`exec_statement`], [`Session`]): `a = 3` and
//! `f(x, y) = ...` define globals (variables are evaluated now; function
//! bodies are kept and resolve global names at call time). Equations with
//! a plot variable on one side (`y = …`, `x = …`, `r = …`) and other
//! equations/inequalities are relations ([`Outcome::Relation`]). See
//! [`StatementKind`] for the classification rules.
//!
//! In scripts (`run_program`, blocks): `let x = v` binds a local; `x = v`
//! assigns a visible local, otherwise defines a global; `fn f(a) { ... }`
//! defines a closure (global at script top level); `if`/`else`, `for x in
//! list`, `while`, `return`. A block evaluates to its last statement.
//! Closures capture visible locals by value and can call themselves.
//!
//! # Budgets and cancellation
//!
//! Every visited node costs one step ([`EvalOptions::max_steps`], default
//! 10 million). Nesting (expression depth + calls) is limited by
//! [`EvalOptions::max_depth`] (default 10 000) *and* by a measured stack
//! budget, [`EvalOptions::max_stack`] (default 1 MiB of machine stack below
//! the frame that created the evaluator — safe on 2 MiB threads even in
//! debug builds, where one user-function recursion level costs ~15 KiB;
//! release frames are much smaller). Runaway loops and deep recursion are
//! therefore errors ([`ErrorKind::StepLimit`], [`ErrorKind::DepthLimit`]),
//! never hangs or stack overflows. The recursive evaluator functions are
//! thin dispatchers over `#[inline(never)]` helpers to keep frames small.
//! [`EvalOptions::interrupt`] is polled every 1024 steps and installed for
//! long number-tower computations.
//!
//! # Builtins
//!
//! Numeric builtins broadcast over lists (and points/matrices where it
//! makes sense).
//!
//! | name | meaning |
//! |------|---------|
//! | `sqrt(x)`, `cbrt(x)`, `root(x, n)` | square, cube and real n-th root (odd roots of negatives are real) |
//! | `abs(x)`, `sign(x)` | absolute value / modulus, sign (`z/|z|` for complex) |
//! | `exp(x)`, `ln(x)` | exponential, natural log |
//! | `log(x)`, `log(x, b)`, `lg(x)`, `log_b(x)` | base 10, base b |
//! | `sin cos tan sec csc cot` | trig (degree mode aware) |
//! | `asin acos atan asec acsc acot` (`arcsin` …) | inverse trig; `acot x = π/2 − atan x` |
//! | `atan2(y, x)` | angle of the point `(x, y)` |
//! | `sinh cosh tanh csch sech coth` | hyperbolic |
//! | `asinh acosh atanh` (`arsinh`, `arcsinh` …) | inverse hyperbolic |
//! | `floor ceil trunc` | rounding to integers |
//! | `round(x)`, `round(x, d)` | nearest (halves away from zero), to `d` decimals |
//! | `frac(x)` | `x − floor(x)` ∈ [0, 1) |
//! | `a mod b` (operator; `mod` is a keyword) | modulo with the sign of `b` |
//! | `gcd(...)`, `lcm(...)` | of numbers or of a list (exact rationals) |
//! | `fact(n)` (also `n!`), `gamma(x)` | factorial (Γ(n+1) for non-integers), gamma |
//! | `binom(n, k)`, `perm(n, k)` | binomial coefficient, k-permutations |
//! | `isprime(n)`, `factor(n)` | primality, prime factors with multiplicity (`factor(12) = [2, 2, 3]`) |
//! | `min(...)`, `max(...)` | of numbers or of a list |
//! | `sum(L)`, `prod(L)` | sum / product of a list |
//! | `len(L)`, `sort(L)`, `reverse(L)` | length, ascending sort, reversal |
//! | `range(a, b)`, `range(a, b, step)` | `[a, a+step, … ≤ b]` (inclusive, exact) |
//! | `num(x)` / `approx(x)` | force a decimal value |
//! | `exact(x)` | simplest fraction within the float's precision (`exact(0.25) = 1/4`) |
//! | `re im conj arg` | complex parts, conjugate, argument |
//! | `transpose(M)` | matrix transpose (a list becomes a column) |

mod builtins;
mod calculus;
mod env;
mod error;
mod interp;
mod ops;
mod plot;
mod statement;
mod value;

#[cfg(test)]
mod tests;

pub use builtins::{ALIASES, BUILTINS, Builtin, BuiltinFn};
pub use env::{AngleUnit, CONSTANTS, Env, EvalOptions, Resolver, canonical_name, is_reserved};
pub use error::{ErrorKind, EvalError, EvalResult};
pub use interp::Evaluator;
pub use plot::{PlotFn, compile_plot_fn};
pub use statement::{
    Outcome, PLOT_VARS, Session, StatementInfo, StatementKind, classify, exec_statement, free_vars,
    run_program,
};
pub use value::{
    FnBody, FormatOptions, Function, Matrix, NumberStyle, UserFn, Value, format_value,
};

use crate::syntax::Expr;

/// Evaluates an expression against `env` without modifying it.
pub fn eval_expr(expr: &Expr, env: &Env, opts: &EvalOptions) -> EvalResult<Value> {
    Evaluator::shared(env, opts).eval(expr)
}
