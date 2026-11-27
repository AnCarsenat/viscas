//! Function catalog: signatures, documentation and examples for
//! completion, signature help and hover docs.
//!
//! Everything goes through [`catalog`]; [`BuiltinCatalog`] adapts the
//! evaluator's builtin table and the curated [`FUNCTION_HELP`] entries.
//! A richer catalog (e.g. the CAS function table) implements [`Catalog`]
//! and replaces the one line in [`catalog`].

use std::sync::OnceLock;

use viscas_core::eval::{ALIASES, BUILTINS};

use super::FUNCTION_HELP;

/// One call form of a function: `log(x, b)` with params `x`, `b`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Display form, e.g. `log(x, b)`.
    pub label: String,
    /// Parameter names in order (a trailing `…` is not a parameter).
    pub params: Vec<String>,
    /// Takes any number of trailing arguments (`gcd(a, b, …)`).
    pub variadic: bool,
}

impl Signature {
    /// Parses a display form such as `log(x, b)` or `gcd(a, b, …)`. A
    /// form without parentheses has no parameters.
    pub fn parse(label: &str) -> Signature {
        let inner = match (label.find('('), label.rfind(')')) {
            (Some(o), Some(c)) if c > o => &label[o + 1..c],
            _ => "",
        };
        let mut variadic = false;
        let params = inner
            .split(',')
            .map(str::trim)
            .filter(|p| {
                let dots = *p == "…" || *p == "...";
                variadic |= dots;
                !p.is_empty() && !dots
            })
            .map(String::from)
            .collect();
        Signature {
            label: label.to_string(),
            params,
            variadic,
        }
    }

    /// `name(p1, p2)` from a name and its parameters.
    pub fn from_params(name: &str, params: &[String]) -> Signature {
        Signature {
            label: format!("{name}({})", params.join(", ")),
            params: params.to_vec(),
            variadic: false,
        }
    }

    /// The parameter argument `arg` binds to (variadic forms repeat their
    /// last parameter); `None` past the end.
    pub fn param_for(&self, arg: usize) -> Option<usize> {
        if arg < self.params.len() {
            Some(arg)
        } else if self.variadic && !self.params.is_empty() {
            Some(self.params.len() - 1)
        } else {
            None
        }
    }

    /// Byte range of parameter `index` inside [`Signature::label`].
    pub fn param_range(&self, index: usize) -> Option<(usize, usize)> {
        let open = self.label.find('(')? + 1;
        let close = self.label.rfind(')')?;
        let mut start = open;
        for (i, part) in self.label[open..close].split(',').enumerate() {
            let lead = part.len() - part.trim_start().len();
            let s = start + lead;
            let e = s + part.trim().len();
            if i == index {
                return Some((s, e));
            }
            start += part.len() + 1;
        }
        None
    }
}

/// Documentation of one function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionDoc {
    pub name: String,
    /// Call forms, fewest parameters first.
    pub signatures: Vec<Signature>,
    /// One-paragraph description.
    pub doc: String,
    /// Inputs showing the function at work (`log(8, 2) = 3`).
    pub examples: Vec<String>,
    /// Grouping such as `trigonometry` or `lists`.
    pub category: String,
}

impl FunctionDoc {
    /// The form with the most parameters (what completion inserts).
    pub fn full_signature(&self) -> Option<&Signature> {
        self.signatures.iter().max_by_key(|s| s.params.len())
    }
}

/// A source of function documentation. Completion, signature help and
/// hover docs only use [`catalog`], so switching catalogs is one line.
pub trait Catalog: Send + Sync {
    /// Every documented function (canonical names, no aliases).
    fn functions(&self) -> Vec<FunctionDoc>;

    /// The function `name` refers to (aliases resolve: `arcsin` → `asin`).
    fn function(&self, name: &str) -> Option<FunctionDoc> {
        let target = resolve_alias(name);
        self.functions().into_iter().find(|f| f.name == target)
    }
}

/// The catalog in use.
pub fn catalog() -> &'static dyn Catalog {
    &BuiltinCatalog
}

fn resolve_alias(name: &str) -> &str {
    ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, t)| *t)
}

/// Catalog over the evaluator's builtin table and [`FUNCTION_HELP`].
pub struct BuiltinCatalog;

impl Catalog for BuiltinCatalog {
    fn functions(&self) -> Vec<FunctionDoc> {
        builtin_docs().to_vec()
    }

    fn function(&self, name: &str) -> Option<FunctionDoc> {
        let target = resolve_alias(name);
        builtin_docs().iter().find(|f| f.name == target).cloned()
    }
}

fn builtin_docs() -> &'static [FunctionDoc] {
    static DOCS: OnceLock<Vec<FunctionDoc>> = OnceLock::new();
    DOCS.get_or_init(|| {
        BUILTINS
            .iter()
            .map(|b| builtin_doc(b.name, b.min, b.max))
            .collect()
    })
}

fn builtin_doc(name: &str, min: usize, max: usize) -> FunctionDoc {
    let entry = FUNCTION_HELP.iter().find(|e| e.name == name);
    let full = match entry {
        Some(e) => Signature::parse(e.signature),
        None => {
            let params: Vec<String> = (0..min.max(1)).map(|i| format!("x{}", i + 1)).collect();
            let mut s = Signature::from_params(name, &params);
            if max == usize::MAX {
                s.variadic = true;
                s.label = format!("{name}({}, …)", params.join(", "));
            }
            s
        }
    };
    let mut signatures = Vec::new();
    // Optional trailing parameters: `log(x)` and `log(x, b)`.
    if !full.variadic && min > 0 && min < full.params.len() {
        signatures.push(Signature::from_params(name, &full.params[..min]));
    }
    signatures.push(full);
    let aliases: Vec<&str> = ALIASES
        .iter()
        .filter(|(_, t)| *t == name)
        .map(|(a, _)| *a)
        .collect();
    let mut doc = match entry {
        Some(e) => {
            let mut s = e.summary.to_string();
            if let Some(first) = s.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            s.push('.');
            s
        }
        None => "Builtin function.".to_string(),
    };
    if !aliases.is_empty() {
        doc.push_str(&format!(" Aliases: {}.", aliases.join(", ")));
    }
    FunctionDoc {
        name: name.to_string(),
        signatures,
        doc,
        examples: examples(name).iter().map(|s| s.to_string()).collect(),
        category: category(name).to_string(),
    }
}

fn category(name: &str) -> &'static str {
    match name {
        "sin" | "cos" | "tan" | "sec" | "csc" | "cot" | "asin" | "acos" | "atan" | "atan2"
        | "asec" | "acsc" | "acot" => "trigonometry",
        "sinh" | "cosh" | "tanh" | "csch" | "sech" | "coth" | "asinh" | "acosh" | "atanh" => {
            "hyperbolic"
        }
        "sqrt" | "cbrt" | "root" | "exp" | "ln" | "log" | "abs" | "sign" => "elementary",
        "floor" | "ceil" | "round" | "trunc" | "frac" => "rounding",
        "gcd" | "lcm" | "fact" | "gamma" | "binom" | "perm" | "isprime" | "factor" => {
            "number theory"
        }
        "min" | "max" | "sum" | "prod" | "len" | "sort" | "reverse" | "range" => "lists",
        "re" | "im" | "conj" | "arg" => "complex",
        "transpose" => "matrices",
        "num" | "exact" => "conversion",
        _ => "other",
    }
}

fn examples(name: &str) -> &'static [&'static str] {
    match name {
        "sqrt" => &["sqrt(8) = 2√2", "√(x^2 + 1)"],
        "cbrt" => &["cbrt(-27) = -3"],
        "root" => &["root(32, 5) = 2"],
        "abs" => &["abs(-3) = 3", "|3 + 4i| = 5"],
        "exp" => &["exp(ln 2) = 2"],
        "ln" => &["ln(e^2) = 2"],
        "log" => &["log(1000) = 3", "log(8, 2) = 3"],
        "sin" => &["sin(π/6) = 1/2", "y = sin x"],
        "cos" => &["cos(π) = -1", "y = cos 2x"],
        "tan" => &["tan(π/4) = 1"],
        "asin" => &["asin(1/2) = π/6"],
        "acos" => &["acos(0) = π/2"],
        "atan" => &["atan(1) = π/4"],
        "atan2" => &["atan2(1, -1) = 3π/4"],
        "floor" => &["floor(2.7) = 2", "floor(-2.5) = -3"],
        "ceil" => &["ceil(2.1) = 3"],
        "round" => &["round(2.5) = 3", "round(π, 2) = 3.14"],
        "gcd" => &["gcd(12, 18) = 6", "gcd(8, 12, 20) = 4"],
        "lcm" => &["lcm(4, 6) = 12"],
        "fact" => &["fact(5) = 120", "5! = 120"],
        "binom" => &["binom(5, 2) = 10"],
        "perm" => &["perm(5, 2) = 20"],
        "isprime" => &["isprime(97) = true"],
        "factor" => &["factor(360)"],
        "min" => &["min(3, 1, 2) = 1", "min([4, 2, 8]) = 2"],
        "max" => &["max(3, 1, 2) = 3"],
        "sum" => &["sum([1, 2, 3]) = 6"],
        "prod" => &["prod([1..5]) = 120"],
        "len" => &["len([1, 2, 3]) = 3"],
        "sort" => &["sort([3, 1, 2]) = [1, 2, 3]"],
        "range" => &["range(1, 10, 3) = [1, 4, 7, 10]"],
        "num" => &["num(1/3) ≈ 0.3333"],
        "exact" => &["exact(0.75) = 3/4"],
        "re" => &["re(3 + 4i) = 3"],
        "im" => &["im(3 + 4i) = 4"],
        "conj" => &["conj(3 + 4i) = 3 - 4i"],
        "arg" => &["arg(i) = π/2"],
        "transpose" => &["transpose([[1, 2], [3, 4]])"],
        "gamma" => &["gamma(5) = 24"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_parse() {
        let s = Signature::parse("log(x, b)");
        assert_eq!(s.params, ["x", "b"]);
        assert!(!s.variadic);
        let g = Signature::parse("gcd(a, b, …)");
        assert_eq!(g.params, ["a", "b"]);
        assert!(g.variadic);
        assert_eq!(g.param_for(5), Some(1));
        assert_eq!(s.param_for(2), None);
        assert!(Signature::parse("π").params.is_empty());
        assert_eq!(s.param_range(1), Some((7, 8)));
        assert_eq!(&s.label[4..5], "x");
        assert_eq!(s.param_range(0), Some((4, 5)));
    }

    #[test]
    fn every_builtin_is_documented() {
        let all = catalog().functions();
        assert_eq!(all.len(), BUILTINS.len());
        for f in &all {
            assert!(!f.signatures.is_empty(), "{}", f.name);
            assert!(!f.doc.is_empty(), "{}", f.name);
        }
    }

    #[test]
    fn optional_params_give_two_forms() {
        let log = catalog().function("log").unwrap();
        let labels: Vec<&str> = log.signatures.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["log(x)", "log(x, b)"]);
        assert_eq!(log.category, "elementary");
        assert!(!log.examples.is_empty());
        assert_eq!(log.full_signature().unwrap().params.len(), 2);
    }

    #[test]
    fn aliases_resolve() {
        let f = catalog().function("arcsin").unwrap();
        assert_eq!(f.name, "asin");
        assert!(f.doc.contains("arcsin"));
        assert!(catalog().function("nope").is_none());
    }
}
