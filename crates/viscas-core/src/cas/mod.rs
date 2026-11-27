//! Computer algebra: symbolic expressions ([`Sym`]) with automatic
//! canonicalization, exact constants, pretty printing, polynomial algebra
//! (expand, factor, cancel, partial fractions), differentiation,
//! simplification, equation/system/inequality solving and step traces.
//!
//! The evaluator wraps a [`Sym`] in `Value::Expr`; see
//! [`crate::eval`] for how symbolic values arise (free symbols inside CAS
//! functions, exact constants `π`/`e`, surds).

pub mod algebra;
pub mod approx;
pub mod convert;
pub mod diff;
pub mod expr;
pub mod mpoly;
pub mod poly;
mod print;
pub mod simplify;
pub mod solve;
mod special;
pub mod steps;

#[cfg(test)]
mod extra_tests;
#[cfg(test)]
mod solve_tests;
#[cfg(test)]
mod tests;

pub use algebra::{
    cancel, coeffs, collect, degree, denominator, expand, factor, numer_denom, numerator, partfrac,
    together,
};
pub use approx::{approx, approx_f64, eval_f64, evalf};
pub use diff::{DiffError, diff, diff_n};
pub use expr::{Const, Func, Node, Sym};
pub use poly::Poly;
pub use simplify::simplify;
pub use steps::{Step, trace};
