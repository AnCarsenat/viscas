//! The function registry: one documented entry per builtin (name,
//! signatures, summary, examples, category, aliases). Help, completion
//! and the command palette read it through [`catalog`] /
//! [`function_doc`]. Every example is evaluated by the test suite, so the
//! shown results are always current.

use super::builtins::ALIASES;
#[cfg(test)]
use super::builtins::BUILTINS;

/// Function categories (for grouping in help and completion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Elementary,
    Trigonometry,
    Hyperbolic,
    Rounding,
    NumberTheory,
    Lists,
    Conversion,
    Complex,
    Matrices,
    Algebra,
    Calculus,
    Solving,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Elementary => "elementary",
            Category::Trigonometry => "trigonometry",
            Category::Hyperbolic => "hyperbolic",
            Category::Rounding => "rounding",
            Category::NumberTheory => "number theory",
            Category::Lists => "lists",
            Category::Conversion => "conversion",
            Category::Complex => "complex numbers",
            Category::Matrices => "matrices",
            Category::Algebra => "algebra",
            Category::Calculus => "calculus",
            Category::Solving => "solving",
        }
    }
}

/// Documentation of one builtin function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnDoc {
    /// Canonical name (as in [`BUILTINS`](super::BUILTINS)).
    pub name: &'static str,
    /// Call forms, most common first (`log(x)`, `log(x, b)`).
    pub signatures: &'static [&'static str],
    /// One-line description.
    pub summary: &'static str,
    /// `(input, result in exact mode)` pairs.
    pub examples: &'static [(&'static str, &'static str)],
    pub category: Category,
    /// Alternative names (`arcsin` for `asin`).
    pub aliases: &'static [&'static str],
    /// CAS function: unknown names in its arguments are symbols.
    pub symbolic: bool,
}

use Category as C;

const fn d(
    name: &'static str,
    signatures: &'static [&'static str],
    summary: &'static str,
    examples: &'static [(&'static str, &'static str)],
    category: Category,
) -> FnDoc {
    FnDoc {
        name,
        signatures,
        summary,
        examples,
        category,
        aliases: &[],
        symbolic: false,
    }
}

const fn cas(
    name: &'static str,
    signatures: &'static [&'static str],
    summary: &'static str,
    examples: &'static [(&'static str, &'static str)],
    category: Category,
) -> FnDoc {
    FnDoc {
        name,
        signatures,
        summary,
        examples,
        category,
        aliases: &[],
        symbolic: true,
    }
}

const fn with_aliases(mut doc: FnDoc, aliases: &'static [&'static str]) -> FnDoc {
    doc.aliases = aliases;
    doc
}

static CATALOG: &[FnDoc] = &[
    // elementary
    d(
        "sqrt",
        &["sqrt(x)", "√x"],
        "square root, exact for rationals (√8 = 2√2)",
        &[("sqrt(8)", "2√2"), ("sqrt(9/4)", "3/2")],
        C::Elementary,
    ),
    d(
        "cbrt",
        &["cbrt(x)", "∛x"],
        "real cube root",
        &[("cbrt(-27)", "-3"), ("cbrt(16)", "2∛2")],
        C::Elementary,
    ),
    d(
        "root",
        &["root(x, n)"],
        "real n-th root (odd roots of negatives are real)",
        &[("root(32, 5)", "2")],
        C::Elementary,
    ),
    d(
        "abs",
        &["abs(x)", "|x|"],
        "absolute value / modulus",
        &[("abs(-3)", "3"), ("abs(3 + 4i)", "5")],
        C::Elementary,
    ),
    d(
        "sign",
        &["sign(x)"],
        "sign: -1, 0 or 1 (z/|z| for complex numbers)",
        &[("sign(-2)", "-1")],
        C::Elementary,
    ),
    d(
        "exp",
        &["exp(x)"],
        "exponential e^x",
        &[("exp(0)", "1"), ("exp(1)", "e")],
        C::Elementary,
    ),
    d(
        "ln",
        &["ln(x)"],
        "natural logarithm",
        &[("ln(e^3)", "3"), ("ln(1)", "0")],
        C::Elementary,
    ),
    with_aliases(
        d(
            "log",
            &["log(x)", "log(x, b)", "log_b(x)"],
            "logarithm in base 10 or base b",
            &[("log(1000)", "3"), ("log(8, 2)", "3")],
            C::Elementary,
        ),
        &["lg"],
    ),
    // trigonometry
    d(
        "sin",
        &["sin(x)"],
        "sine; exact at rational multiples of π (degree mode aware)",
        &[("sin(π/6)", "1/2"), ("sin(π/4)", "√2/2")],
        C::Trigonometry,
    ),
    d(
        "cos",
        &["cos(x)"],
        "cosine; exact at rational multiples of π (degree mode aware)",
        &[("cos(π)", "-1"), ("cos(π/6)", "√3/2")],
        C::Trigonometry,
    ),
    d(
        "tan",
        &["tan(x)"],
        "tangent (degree mode aware)",
        &[("tan(π/3)", "√3")],
        C::Trigonometry,
    ),
    d(
        "sec",
        &["sec(x)"],
        "secant 1/cos x",
        &[("sec(0)", "1")],
        C::Trigonometry,
    ),
    d(
        "csc",
        &["csc(x)"],
        "cosecant 1/sin x",
        &[("csc(π/2)", "1")],
        C::Trigonometry,
    ),
    d(
        "cot",
        &["cot(x)"],
        "cotangent 1/tan x",
        &[("cot(π/4)", "1")],
        C::Trigonometry,
    ),
    with_aliases(
        d(
            "asin",
            &["asin(x)"],
            "inverse sine",
            &[("asin(1/2)", "π/6")],
            C::Trigonometry,
        ),
        &["arcsin"],
    ),
    with_aliases(
        d(
            "acos",
            &["acos(x)"],
            "inverse cosine",
            &[("acos(0)", "π/2")],
            C::Trigonometry,
        ),
        &["arccos"],
    ),
    with_aliases(
        d(
            "atan",
            &["atan(x)"],
            "inverse tangent",
            &[("atan(1)", "π/4")],
            C::Trigonometry,
        ),
        &["arctan"],
    ),
    d(
        "atan2",
        &["atan2(y, x)"],
        "angle of the point (x, y)",
        &[("atan2(1, 1)", "≈0.785398163397")],
        C::Trigonometry,
    ),
    with_aliases(
        d(
            "asec",
            &["asec(x)"],
            "inverse secant",
            &[("asec(1)", "0")],
            C::Trigonometry,
        ),
        &["arcsec"],
    ),
    with_aliases(
        d(
            "acsc",
            &["acsc(x)"],
            "inverse cosecant",
            &[("acsc(1)", "π/2")],
            C::Trigonometry,
        ),
        &["arccsc"],
    ),
    with_aliases(
        d(
            "acot",
            &["acot(x)"],
            "inverse cotangent π/2 − atan x",
            &[("acot(1)", "π/4")],
            C::Trigonometry,
        ),
        &["arccot"],
    ),
    // hyperbolic
    d(
        "sinh",
        &["sinh(x)"],
        "hyperbolic sine",
        &[("sinh(0)", "0")],
        C::Hyperbolic,
    ),
    d(
        "cosh",
        &["cosh(x)"],
        "hyperbolic cosine",
        &[("cosh(0)", "1")],
        C::Hyperbolic,
    ),
    d(
        "tanh",
        &["tanh(x)"],
        "hyperbolic tangent",
        &[("tanh(0)", "0")],
        C::Hyperbolic,
    ),
    d(
        "csch",
        &["csch(x)"],
        "hyperbolic cosecant 1/sinh x",
        &[("csch(1)", "≈0.850918128239")],
        C::Hyperbolic,
    ),
    d(
        "sech",
        &["sech(x)"],
        "hyperbolic secant 1/cosh x",
        &[("sech(0)", "1")],
        C::Hyperbolic,
    ),
    d(
        "coth",
        &["coth(x)"],
        "hyperbolic cotangent 1/tanh x",
        &[("coth(1)", "≈1.3130352855")],
        C::Hyperbolic,
    ),
    with_aliases(
        d(
            "asinh",
            &["asinh(x)"],
            "inverse hyperbolic sine",
            &[("asinh(0)", "0")],
            C::Hyperbolic,
        ),
        &["arsinh", "arcsinh"],
    ),
    with_aliases(
        d(
            "acosh",
            &["acosh(x)"],
            "inverse hyperbolic cosine",
            &[("acosh(1)", "0")],
            C::Hyperbolic,
        ),
        &["arcosh", "arccosh"],
    ),
    with_aliases(
        d(
            "atanh",
            &["atanh(x)"],
            "inverse hyperbolic tangent",
            &[("atanh(0)", "0")],
            C::Hyperbolic,
        ),
        &["artanh", "arctanh"],
    ),
    // rounding and integers
    d(
        "floor",
        &["floor(x)"],
        "largest integer ≤ x",
        &[("floor(7/2)", "3"), ("floor(-π)", "-4")],
        C::Rounding,
    ),
    d(
        "ceil",
        &["ceil(x)"],
        "smallest integer ≥ x",
        &[("ceil(7/2)", "4")],
        C::Rounding,
    ),
    d(
        "round",
        &["round(x)", "round(x, d)"],
        "nearest integer (halves away from zero) or d decimals",
        &[("round(5/2)", "3"), ("round(π, 2)", "≈3.14")],
        C::Rounding,
    ),
    d(
        "trunc",
        &["trunc(x)"],
        "integer part (towards zero)",
        &[("trunc(-7/2)", "-3")],
        C::Rounding,
    ),
    d(
        "frac",
        &["frac(x)"],
        "fractional part x − floor(x) ∈ [0, 1)",
        &[("frac(7/2)", "1/2")],
        C::Rounding,
    ),
    d(
        "gcd",
        &["gcd(a, b, …)", "gcd(L)"],
        "greatest common divisor (exact rationals)",
        &[("gcd(12, 18)", "6")],
        C::NumberTheory,
    ),
    d(
        "lcm",
        &["lcm(a, b, …)", "lcm(L)"],
        "least common multiple",
        &[("lcm(4, 6)", "12")],
        C::NumberTheory,
    ),
    d(
        "fact",
        &["fact(n)", "n!"],
        "factorial (Γ(n+1) for non-integers)",
        &[("fact(5)", "120")],
        C::NumberTheory,
    ),
    d(
        "gamma",
        &["gamma(x)"],
        "gamma function Γ(x)",
        &[("gamma(5)", "24"), ("gamma(1/2)", "√π")],
        C::NumberTheory,
    ),
    d(
        "binom",
        &["binom(n, k)"],
        "binomial coefficient",
        &[("binom(5, 2)", "10")],
        C::NumberTheory,
    ),
    d(
        "perm",
        &["perm(n, k)"],
        "number of k-permutations of n",
        &[("perm(5, 2)", "20")],
        C::NumberTheory,
    ),
    d(
        "isprime",
        &["isprime(n)"],
        "primality test",
        &[("isprime(97)", "true")],
        C::NumberTheory,
    ),
    cas(
        "factor",
        &["factor(n)", "factor(expr)"],
        "prime factors of an integer, or factorization of a polynomial / rational function over ℚ",
        &[
            ("factor(12)", "[2, 2, 3]"),
            ("factor(x^2 - 1)", "(x - 1)(x + 1)"),
            ("factor(x^3 - 6x^2 + 11x - 6)", "(x - 3)(x - 2)(x - 1)"),
        ],
        C::Algebra,
    ),
    // lists
    d(
        "min",
        &["min(a, b, …)", "min(L)"],
        "minimum of numbers or a list",
        &[("min(3, 1, 2)", "1")],
        C::Lists,
    ),
    d(
        "max",
        &["max(a, b, …)", "max(L)"],
        "maximum of numbers or a list",
        &[("max([3, 1, 2])", "3")],
        C::Lists,
    ),
    d(
        "sum",
        &["sum(L)", "sum(k = a, b) expr"],
        "sum of a list (or ∑ with bounds)",
        &[("sum([1, 2, 3])", "6")],
        C::Lists,
    ),
    d(
        "prod",
        &["prod(L)", "prod(k = a, b) expr"],
        "product of a list (or ∏ with bounds)",
        &[("prod([1, 2, 3, 4])", "24")],
        C::Lists,
    ),
    d(
        "len",
        &["len(L)"],
        "length of a list",
        &[("len([1, 2, 3])", "3")],
        C::Lists,
    ),
    d(
        "sort",
        &["sort(L)"],
        "ascending sort",
        &[("sort([3, 1, 2])", "[1, 2, 3]")],
        C::Lists,
    ),
    d(
        "reverse",
        &["reverse(L)"],
        "reversed list",
        &[("reverse([1, 2, 3])", "[3, 2, 1]")],
        C::Lists,
    ),
    d(
        "range",
        &["range(a, b)", "range(a, b, step)"],
        "inclusive exact range [a, a+step, … ≤ b]",
        &[("range(1, 2, 1/2)", "[1, 3/2, 2]")],
        C::Lists,
    ),
    // conversion and complex
    with_aliases(
        d(
            "num",
            &["num(x)"],
            "decimal value (numeric approximation)",
            &[("num(1/4)", "≈0.25"), ("num(π)", "≈3.14159265359")],
            C::Conversion,
        ),
        &["approx"],
    ),
    d(
        "exact",
        &["exact(x)"],
        "simplest fraction within the float's precision",
        &[("exact(0.25)", "1/4")],
        C::Conversion,
    ),
    d(
        "re",
        &["re(z)"],
        "real part",
        &[("re(3 + 4i)", "3")],
        C::Complex,
    ),
    d(
        "im",
        &["im(z)"],
        "imaginary part",
        &[("im(3 + 4i)", "4")],
        C::Complex,
    ),
    d(
        "conj",
        &["conj(z)"],
        "complex conjugate",
        &[("conj(3 + 4i)", "3 - 4i")],
        C::Complex,
    ),
    d(
        "arg",
        &["arg(z)"],
        "argument (angle) of a complex number",
        &[("arg(1)", "0")],
        C::Complex,
    ),
    d(
        "transpose",
        &["transpose(M)", "M.T"],
        "matrix transpose",
        &[("transpose([[1, 2], [3, 4]])", "[[1, 3], [2, 4]]")],
        C::Matrices,
    ),
    // computer algebra
    cas(
        "simplify",
        &["simplify(expr)"],
        "simplest form: cancels fractions, combines terms, trig/log/root identities",
        &[
            ("simplify(sin(x)^2 + cos(x)^2)", "1"),
            ("simplify((x^2 - 1)/(x + 1))", "x - 1"),
        ],
        C::Algebra,
    ),
    cas(
        "expand",
        &["expand(expr)"],
        "multiplies out products and integer powers",
        &[("expand((x + 1)^2)", "x^2 + 2x + 1")],
        C::Algebra,
    ),
    cas(
        "together",
        &["together(expr)"],
        "combines into one fraction",
        &[("together(1/x + 1/y)", "(x + y)/(x·y)")],
        C::Algebra,
    ),
    cas(
        "cancel",
        &["cancel(expr)"],
        "cancels common factors of numerator and denominator",
        &[("cancel((x^2 - 1)/(x - 1))", "x + 1")],
        C::Algebra,
    ),
    cas(
        "numerator",
        &["numerator(expr)"],
        "numerator of a fraction",
        &[("numerator((x + 1)/(x - 1))", "x + 1")],
        C::Algebra,
    ),
    cas(
        "denominator",
        &["denominator(expr)"],
        "denominator of a fraction",
        &[("denominator((x + 1)/(x - 1))", "x - 1")],
        C::Algebra,
    ),
    cas(
        "collect",
        &["collect(expr, x)"],
        "groups the terms by powers of x",
        &[("collect(a x + b x + c, x)", "(a + b)·x + c")],
        C::Algebra,
    ),
    cas(
        "coeffs",
        &["coeffs(p)", "coeffs(p, x)"],
        "polynomial coefficients, highest degree first",
        &[("coeffs(x^2 + 2x + 3)", "[1, 2, 3]")],
        C::Algebra,
    ),
    cas(
        "degree",
        &["degree(p)", "degree(p, x)"],
        "degree of a polynomial",
        &[("degree(x^3 + x)", "3")],
        C::Algebra,
    ),
    cas(
        "partfrac",
        &["partfrac(expr)", "partfrac(expr, x)"],
        "partial fraction decomposition over ℚ",
        &[("partfrac(1/(x^2 - 1))", "1/(2(x - 1)) - 1/(2(x + 1))")],
        C::Algebra,
    ),
    cas(
        "subs",
        &["subs(expr, x, value)", "subs(expr, x = value)"],
        "substitutes a value for a variable",
        &[("subs(x^2 + 1, x, 3)", "10")],
        C::Algebra,
    ),
    cas(
        "diff",
        &[
            "diff(expr, x)",
            "diff(expr, x, n)",
            "diff(expr, x, y)",
            "diff(f)",
        ],
        "symbolic derivative (n-th, or mixed partial)",
        &[
            ("diff(x^3, x)", "3x^2"),
            ("diff(sin(x^2), x)", "2x·cos(x^2)"),
            ("diff(x^4, x, 2)", "12x^2"),
        ],
        C::Calculus,
    ),
    cas(
        "solve",
        &[
            "solve(eq, x)",
            "solve(eq, x, a, b)",
            "solve([eq1, eq2], [x, y])",
            "solve(ineq, x)",
        ],
        "exact solutions of equations, systems and inequalities; a, b restrict to a domain",
        &[
            ("solve(x^2 = 4, x)", "[-2, 2]"),
            ("solve(x^2 - 2 = 0, x)", "[-√2, √2]"),
            ("solve(x^2 < 4, x)", "[-2 < x < 2]"),
            ("solve([x + y = 3, x - y = 1], [x, y])", "[[x = 2, y = 1]]"),
            ("solve(sin(x) = 1/2, x, 0, 2π)", "[π/6, 5π/6]"),
        ],
        C::Solving,
    ),
    cas(
        "nsolve",
        &["nsolve(eq, x, guess)", "nsolve(eq, x, a, b)"],
        "numeric root from a starting guess, or all roots in [a, b]",
        &[("nsolve(cos(x) = x, x, 1)", "≈0.739085133215")],
        C::Solving,
    ),
    cas(
        "roots",
        &["roots(p)", "roots(p, x)"],
        "all roots of a polynomial with multiplicity",
        &[("roots(x^3 - x)", "[-1, 0, 1]")],
        C::Solving,
    ),
    cas(
        "steps",
        &["steps(expr)"],
        "the rewrite steps of a CAS computation (step-by-step)",
        &[],
        C::Algebra,
    ),
];

/// Every builtin with its documentation.
pub fn catalog() -> &'static [FnDoc] {
    CATALOG
}

/// Documentation of a builtin by canonical name or alias.
pub fn function_doc(name: &str) -> Option<&'static FnDoc> {
    let target = ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, t)| *t);
    CATALOG.iter().find(|d| d.name == target)
}

/// Every builtin name has an entry (checked by the tests).
#[cfg(test)]
pub(crate) fn missing_docs() -> Vec<&'static str> {
    BUILTINS
        .iter()
        .filter(|b| !CATALOG.iter().any(|d| d.name == b.name))
        .map(|b| b.name)
        .collect()
}
