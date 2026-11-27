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
//! [`Matrix`], a function (user closure or builtin), `Undefined`, or a
//! symbolic expression (`Value::Expr`, see [`crate::cas`]): `π`, `√2`,
//! `2x + 1`, `x = 2`. The enum is non-exhaustive.
//!
//! **Exact first.** In exact mode `π`, `e`, `τ`, `φ` are exact symbolic
//! constants and surds stay exact (`√8 = 2√2`, `sin(π/4) = √2/2`,
//! `asin(1/2) = π/6`); transcendental functions of plain rationals
//! without a special value (`sin(1)`) are numeric. Constant expressions
//! turn into numbers wherever a number is needed (`Value::as_number`
//! gives their approximation). Inside the arguments of CAS functions
//! (`simplify`, `solve`, `diff`, …) unknown names are symbols. See the
//! glue module `symbolic.rs` for the exact rules and [`catalog`] for the
//! documented function list (used by help and completion).
//!
//! - A list literal whose elements are equal-length lists of numbers is a
//!   matrix (`[[1, 2], [3, 4]]`); `*` between matrices is the matrix
//!   product, matrix × list is matrix × vector, `M^n` (n ≥ 0) repeated
//!   product, `M.T` / `transpose(M)` the transpose.
//! - `Undefined` comes from failed restrictions and piecewise expressions
//!   without a matching branch; it propagates through arithmetic and counts
//!   as false in conditions. Numeric failures (division by zero, domain
//!   errors) are *errors* with a span. The plot path turns both into NaN
//!   but keeps them apart (an "undefined" NaN payload counts as false in
//!   conditions, an error NaN — including infinities — poisons the result),
//!   matching the general evaluator.
//! - Exact-style formatting marks every inexact number with `≈`
//!   (`≈3.14159265359`), so numeric results never claim exactness.
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
//! - **Calculus.** `∑`/`∏` with integer bounds are exact loops. `d/dx`
//!   and `f'(x)` are symbolic in exact mode (`d/dx x^3 = 3x^2`, evaluated
//!   exactly at the current value of `x` if it has one), numeric (5-point
//!   central differences) in `float64` mode or when the body has no
//!   symbolic derivative. Definite `∫` is numeric (adaptive Gauss–Kronrod
//!   7/15, infinite bounds by substitution). Indefinite integrals and
//!   infinite sums need the CAS integration stage (hooks in
//!   `calculus.rs`).
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
//! shadowed by parameters and `let`). Neither can the plot variables
//! [`PLOT_VARS`] (`x y r θ t`): they are always free in statement analysis
//! unless bound by a parameter, a binder (`∑`, `∫`, comprehension) or an
//! explicit `let`.
//!
//! **Call vs multiply.** `name(args)` calls `name` if it resolves to a
//! function; otherwise it multiplies (`a(b + 1)` = `a·(b + 1)`,
//! `a(1, 2)` = `a·(1, 2)`). For an unknown split name the last piece is
//! applied (`af(x)` = `a·f(x)`). `log_2(x)` / `log_b x` use the subscript
//! as the base.
//!
//! # Statements
//!
//! Definitions use `:=` (see [`DefinitionOp`](crate::syntax::DefinitionOp)):
//!
//! | statement | meaning (default `ColonEq`) |
//! |-----------|-----------------------------|
//! | `a := 3`, `P := (1, 2)` | define a global variable (evaluated now) |
//! | `f(x, y) := ...` | define a global function (body resolves globals at call time) |
//! | `y = x^2`, `x = y^2`, `r = θ` | explicit / polar relation |
//! | `x^2 + y^2 = 4`, `xy = 1` | implicit relation |
//! | `a = 3`, `t^2 = 2` | equation without plot coordinates ([`StatementKind::Equation`], future solve target) |
//! | `x^2 + y^2 < 4` | inequality |
//! | `r < 2`, `0 < θ < π/2` | polar inequality |
//! | `θ = π/4` | polar angle relation |
//!
//! At top level ([`exec_statement`], [`Session`]) every `=` statement is a
//! relation ([`Outcome::Relation`]) — except `v = ...` for a visible `let`
//! variable `v`, which reassigns it; [`Session::eval`] reports a relation
//! as having no value. [`Session::run`] returns one [`Outcome`] per
//! statement, and the session keeps its `let` locals between inputs. In
//! approx mode only the *returned* value is converted to decimals; stored
//! definitions stay exact. With `DefinitionOp::Eq` (set [`Session::parse`] or use
//! [`crate::syntax::parse_statement_with`]) `name = ...` / `f(x) = ...`
//! define as before, except for `x`, `y`, `r` and prefix functions. See
//! [`StatementKind`] for the classification rules.
//!
//! In scripts (`run_program`, blocks): `let x = v` declares a local and
//! `x = v` **reassigns a visible `let` variable** (any other `=` is an
//! equation and an error in a script); `g := v` / `h(x) := ...` always
//! define globals (shadowing: inside the scope, a visible `let g` still
//! wins over the global `g`; only `DefinitionOp::Eq`'s `g = v` reassigns
//! the `let` instead);
//! `fn f(a) { ... }` defines a closure (global at script top level);
//! `if`/`else`, `for x in list`, `while`, `return`. A block evaluates to
//! its last statement. Closures capture visible locals by value and can
//! call themselves.
//!
//! # Budgets and cancellation
//!
//! Every visited node costs one step ([`EvalOptions::max_steps`], default
//! 10 million); list operations, matrix products and ranges charge per
//! element, and ranges/comprehensions share an element budget
//! ([`EvalOptions::max_elements`], default 10 million). Lists and points
//! may nest at most 32 levels, which keeps every recursive walk over
//! values (clone, drop, display, broadcasting) shallow. Nesting (expression depth + calls) is limited by
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
//! | `simplify expand factor together cancel` | CAS rewriting (`factor(n)` on an integer: prime factors) |
//! | `numerator denominator collect coeffs degree partfrac subs` | rational functions and polynomials |
//! | `diff(f, x[, n])` | symbolic derivative |
//! | `solve(eq, x[, a, b])`, `solve([eqs], [vars])` | exact solving: equations, systems, inequalities |
//! | `nsolve(eq, x, guess)`, `nsolve(eq, x, a, b)`, `roots(p)` | numeric roots, polynomial roots |
//! | `steps(expr)` | the CAS rewrite steps of `expr` |

mod builtins;
mod calculus;
mod catalog;
mod env;
mod error;
mod interp;
mod ops;
mod plot;
mod statement;
mod symbolic;
mod value;

#[cfg(test)]
mod cas_tests;
#[cfg(test)]
mod tests;

pub use builtins::{ALIASES, BUILTINS, Builtin, BuiltinFn};
pub use catalog::{Category, FnDoc, catalog, function_doc};
pub use env::{AngleUnit, CONSTANTS, Env, EvalOptions, Resolver, canonical_name, is_reserved};
pub use error::{ErrorKind, EvalError, EvalResult};
pub use interp::Evaluator;
pub use plot::{PlotFn, compile_plot_fn};
pub use statement::{
    Outcome, PLOT_VARS, Session, StatementInfo, StatementKind, classify, exec_statement, free_vars,
    run_program,
};
pub use value::{
    FnBody, FormatOptions, Function, Matrix, NumberStyle, SymValue, UserFn, Value, format_value,
};

use crate::syntax::Expr;

/// Evaluates an expression against `env` without modifying it.
pub fn eval_expr(expr: &Expr, env: &Env, opts: &EvalOptions) -> EvalResult<Value> {
    Evaluator::shared(env, opts).eval(expr)
}
