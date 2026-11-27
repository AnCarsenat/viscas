//! Short reference texts for builtins, constants and console commands
//! (`:help name`, completion details, command palette).

use viscas_core::eval::{ALIASES, BUILTINS, CONSTANTS, canonical_name};

/// One help entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpEntry {
    pub name: &'static str,
    /// Call signature (`log(x, b)`) or the name itself.
    pub signature: &'static str,
    pub summary: &'static str,
}

const fn h(name: &'static str, signature: &'static str, summary: &'static str) -> HelpEntry {
    HelpEntry {
        name,
        signature,
        summary,
    }
}

/// Builtin functions.
pub static FUNCTION_HELP: &[HelpEntry] = &[
    h("sqrt", "sqrt(x)", "square root (also √x)"),
    h("cbrt", "cbrt(x)", "real cube root"),
    h("root", "root(x, n)", "real n-th root"),
    h("abs", "abs(x)", "absolute value / modulus (also |x|)"),
    h("sign", "sign(x)", "sign (z/|z| for complex numbers)"),
    h("exp", "exp(x)", "exponential e^x"),
    h("ln", "ln(x)", "natural logarithm"),
    h(
        "log",
        "log(x, b)",
        "logarithm, base 10 or base b (also log_b x)",
    ),
    h("sin", "sin(x)", "sine (degree mode aware)"),
    h("cos", "cos(x)", "cosine (degree mode aware)"),
    h("tan", "tan(x)", "tangent (degree mode aware)"),
    h("sec", "sec(x)", "secant 1/cos x"),
    h("csc", "csc(x)", "cosecant 1/sin x"),
    h("cot", "cot(x)", "cotangent 1/tan x"),
    h("asin", "asin(x)", "inverse sine (also arcsin)"),
    h("acos", "acos(x)", "inverse cosine (also arccos)"),
    h("atan", "atan(x)", "inverse tangent (also arctan)"),
    h("atan2", "atan2(y, x)", "angle of the point (x, y)"),
    h("asec", "asec(x)", "inverse secant"),
    h("acsc", "acsc(x)", "inverse cosecant"),
    h("acot", "acot(x)", "inverse cotangent π/2 − atan x"),
    h("sinh", "sinh(x)", "hyperbolic sine"),
    h("cosh", "cosh(x)", "hyperbolic cosine"),
    h("tanh", "tanh(x)", "hyperbolic tangent"),
    h("csch", "csch(x)", "hyperbolic cosecant"),
    h("sech", "sech(x)", "hyperbolic secant"),
    h("coth", "coth(x)", "hyperbolic cotangent"),
    h("asinh", "asinh(x)", "inverse hyperbolic sine"),
    h("acosh", "acosh(x)", "inverse hyperbolic cosine"),
    h("atanh", "atanh(x)", "inverse hyperbolic tangent"),
    h("floor", "floor(x)", "largest integer ≤ x"),
    h("ceil", "ceil(x)", "smallest integer ≥ x"),
    h(
        "round",
        "round(x, d)",
        "nearest integer (halves away from zero) or d decimals",
    ),
    h("trunc", "trunc(x)", "integer part (towards zero)"),
    h("frac", "frac(x)", "x − floor(x)"),
    h("gcd", "gcd(a, b, …)", "greatest common divisor"),
    h("lcm", "lcm(a, b, …)", "least common multiple"),
    h("fact", "fact(n)", "factorial (also n!)"),
    h("gamma", "gamma(x)", "gamma function Γ(x)"),
    h("binom", "binom(n, k)", "binomial coefficient"),
    h("perm", "perm(n, k)", "k-permutations of n"),
    h("isprime", "isprime(n)", "primality test"),
    h(
        "factor",
        "factor(n)",
        "prime factors of an integer, or factorization of a polynomial",
    ),
    h("min", "min(a, b, …)", "minimum of numbers or a list"),
    h("max", "max(a, b, …)", "maximum of numbers or a list"),
    h("sum", "sum(L)", "sum of a list"),
    h("prod", "prod(L)", "product of a list"),
    h("len", "len(L)", "length of a list"),
    h("sort", "sort(L)", "ascending sort"),
    h("reverse", "reverse(L)", "reversed list"),
    h(
        "range",
        "range(a, b, step)",
        "inclusive exact range [a, a+step, … ≤ b]",
    ),
    h("num", "num(x)", "force a decimal value (also approx)"),
    h(
        "exact",
        "exact(x)",
        "simplest fraction within the float's precision",
    ),
    h("re", "re(z)", "real part"),
    h("im", "im(z)", "imaginary part"),
    h("conj", "conj(z)", "complex conjugate"),
    h("arg", "arg(z)", "argument (angle) of a complex number"),
    h("transpose", "transpose(M)", "matrix transpose"),
    // computer algebra (full docs: viscas_core::eval::catalog())
    h(
        "simplify",
        "simplify(expr)",
        "simplest form (fractions, trig/log/root identities)",
    ),
    h("expand", "expand(expr)", "multiply out products and powers"),
    h("together", "together(expr)", "combine into one fraction"),
    h(
        "cancel",
        "cancel(expr)",
        "cancel common factors of a fraction",
    ),
    h("numerator", "numerator(expr)", "numerator of a fraction"),
    h(
        "denominator",
        "denominator(expr)",
        "denominator of a fraction",
    ),
    h("collect", "collect(expr, x)", "group terms by powers of x"),
    h(
        "coeffs",
        "coeffs(p, x)",
        "polynomial coefficients, highest degree first",
    ),
    h("degree", "degree(p, x)", "degree of a polynomial"),
    h(
        "partfrac",
        "partfrac(expr, x)",
        "partial fraction decomposition",
    ),
    h(
        "subs",
        "subs(expr, x, value)",
        "substitute a value for a variable",
    ),
    h("diff", "diff(expr, x, n)", "symbolic derivative"),
    h(
        "solve",
        "solve(eq, x, a, b)",
        "exact solutions of equations, systems, inequalities",
    ),
    h(
        "nsolve",
        "nsolve(eq, x, a, b)",
        "numeric roots (from a guess or in [a, b])",
    ),
    h(
        "roots",
        "roots(p, x)",
        "roots of a polynomial with multiplicity",
    ),
    h(
        "steps",
        "steps(expr)",
        "step-by-step rewrite steps of a CAS computation",
    ),
];

/// Constants (canonical names).
pub static CONSTANT_HELP: &[HelpEntry] = &[
    h("π", "π", "pi ≈ 3.14159 (also pi)"),
    h("τ", "τ", "tau = 2π (also tau)"),
    h("e", "e", "Euler's number ≈ 2.71828"),
    h("φ", "φ", "golden ratio ≈ 1.61803 (also phi)"),
    h("i", "i", "imaginary unit"),
    h("∞", "∞", "infinity, only as an integral bound (also inf)"),
    h("true", "true", "boolean true"),
    h("false", "false", "boolean false"),
];

/// Console commands.
pub static COMMAND_HELP: &[HelpEntry] = &[
    h(
        ":help",
        ":help name",
        "describe a function, constant, node or command",
    ),
    h(
        ":mode",
        ":mode exact|approx|deg|rad|real|complex",
        "switch evaluation mode",
    ),
    h(
        ":add",
        ":add",
        "add the last input to the board as a node (also :pin, Ctrl+Shift+Enter)",
    ),
    h(
        ":plot",
        ":plot input",
        "evaluate the input and add it to the board (y = x^2, (1, 2)); a := 5 updates an existing node a",
    ),
    h(
        ":pin",
        ":pin",
        "alias of :add: the last input becomes a board node",
    ),
    h(
        ":clear",
        ":clear",
        "clear the console transcript and its local definitions",
    ),
    h(":vars", ":vars", "list scene and console definitions"),
];

/// ASCII spellings of the Greek constants, offered by completion.
pub static ASCII_ALIASES: &[&str] = &["pi", "tau", "phi", "theta", "inf"];

/// Help for a builtin function or alias (`arcsin` → `asin`).
pub fn function_help(name: &str) -> Option<&'static HelpEntry> {
    let target = ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, t)| *t);
    FUNCTION_HELP.iter().find(|e| e.name == target).or_else(|| {
        // A builtin without a curated entry still gets a generic one.
        BUILTINS.iter().find(|b| b.name == target).map(|_| &GENERIC)
    })
}

static GENERIC: HelpEntry = h("", "", "builtin function");

/// Help for a constant (`pi` → `π`).
pub fn constant_help(name: &str) -> Option<&'static HelpEntry> {
    let name = canonical_name(name);
    if !CONSTANTS.contains(&name) {
        return None;
    }
    CONSTANT_HELP.iter().find(|e| e.name == name)
}

/// Help for a console command (with or without the leading `:`).
pub fn command_help(name: &str) -> Option<&'static HelpEntry> {
    let name = name.strip_prefix(':').unwrap_or(name);
    COMMAND_HELP.iter().find(|e| &e.name[1..] == name)
}
