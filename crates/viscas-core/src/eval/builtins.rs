//! The core builtin function library. See the table in the
//! [module docs](super).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

use super::env::AngleUnit;
use super::error::{ErrorKind, EvalError, EvalResult, NumSpan};
use super::interp::Evaluator;
use super::ops::Arith;
use super::symbolic as sy;
use super::value::{Function, Matrix, Value};
use crate::number::{Number, NumberError, Precision};
use crate::syntax::Span;

/// Implementation of a builtin: evaluator, evaluated arguments (count
/// already checked), call span.
pub type BuiltinFn = fn(&mut Evaluator<'_>, Vec<Value>, Span) -> EvalResult<Value>;

/// A builtin function.
#[derive(Debug)]
pub struct Builtin {
    pub name: &'static str,
    pub min: usize,
    /// `usize::MAX` for variadic functions.
    pub max: usize,
    pub f: BuiltinFn,
    /// CAS function: arguments are evaluated in symbolic mode (unknown
    /// names are symbols).
    pub symbolic: bool,
    /// Argument positions whose bare names (or lists of names) are
    /// variables, bound to symbols while the arguments are evaluated
    /// (`x` in `diff(f, x)`).
    pub vars: &'static [usize],
}

const VAR: usize = usize::MAX;

macro_rules! b {
    ($name:literal, $min:expr, $max:expr, $f:expr) => {
        Builtin {
            name: $name,
            min: $min,
            max: $max,
            f: $f,
            symbolic: false,
            vars: &[],
        }
    };
}

/// CAS builtin (symbolic arguments).
macro_rules! c {
    ($name:literal, $min:expr, $max:expr, $f:expr, $vars:expr) => {
        Builtin {
            name: $name,
            min: $min,
            max: $max,
            f: $f,
            symbolic: true,
            vars: $vars,
        }
    };
}

/// Every builtin, by canonical name.
pub static BUILTINS: &[Builtin] = &[
    // elementary
    b!("sqrt", 1, 1, b_sqrt),
    b!("cbrt", 1, 1, b_cbrt),
    b!("root", 2, 2, b_root),
    b!("abs", 1, 1, b_abs),
    b!("sign", 1, 1, b_sign),
    b!("exp", 1, 1, b_exp),
    b!("ln", 1, 1, b_ln),
    b!("log", 1, 2, b_log),
    // trigonometry
    b!("sin", 1, 1, b_sin),
    b!("cos", 1, 1, b_cos),
    b!("tan", 1, 1, b_tan),
    b!("sec", 1, 1, b_sec),
    b!("csc", 1, 1, b_csc),
    b!("cot", 1, 1, b_cot),
    b!("asin", 1, 1, b_asin),
    b!("acos", 1, 1, b_acos),
    b!("atan", 1, 1, b_atan),
    b!("atan2", 2, 2, b_atan2),
    b!("asec", 1, 1, b_asec),
    b!("acsc", 1, 1, b_acsc),
    b!("acot", 1, 1, b_acot),
    // hyperbolic
    b!("sinh", 1, 1, b_sinh),
    b!("cosh", 1, 1, b_cosh),
    b!("tanh", 1, 1, b_tanh),
    b!("csch", 1, 1, b_csch),
    b!("sech", 1, 1, b_sech),
    b!("coth", 1, 1, b_coth),
    b!("asinh", 1, 1, b_asinh),
    b!("acosh", 1, 1, b_acosh),
    b!("atanh", 1, 1, b_atanh),
    // rounding and integers
    b!("floor", 1, 1, b_floor),
    b!("ceil", 1, 1, b_ceil),
    b!("round", 1, 2, b_round),
    b!("trunc", 1, 1, b_trunc),
    b!("frac", 1, 1, b_frac),
    b!("gcd", 1, VAR, b_gcd),
    b!("lcm", 1, VAR, b_lcm),
    b!("fact", 1, 1, b_fact),
    b!("gamma", 1, 1, b_gamma),
    b!("binom", 2, 2, b_binom),
    b!("perm", 2, 2, b_perm),
    b!("isprime", 1, 1, b_isprime),
    c!("factor", 1, 1, sy::b_factor, &[]),
    // lists
    b!("min", 1, VAR, b_min),
    b!("max", 1, VAR, b_max),
    b!("sum", 1, 1, b_sum),
    b!("prod", 1, 1, b_prod),
    b!("len", 1, 1, b_len),
    b!("sort", 1, 1, b_sort),
    b!("reverse", 1, 1, b_reverse),
    b!("range", 2, 3, b_range),
    // conversion and complex numbers
    b!("num", 1, 1, b_num),
    b!("exact", 1, 1, b_exact),
    b!("re", 1, 1, b_re),
    b!("im", 1, 1, b_im),
    b!("conj", 1, 1, b_conj),
    b!("arg", 1, 1, b_arg),
    // matrices
    b!("transpose", 1, 1, b_transpose),
    // computer algebra
    c!("simplify", 1, 1, sy::b_simplify, &[]),
    c!("expand", 1, 1, sy::b_expand, &[]),
    c!("together", 1, 1, sy::b_together, &[]),
    c!("cancel", 1, 1, sy::b_cancel, &[]),
    c!("numerator", 1, 1, sy::b_numerator, &[]),
    c!("denominator", 1, 1, sy::b_denominator, &[]),
    c!("collect", 2, 2, sy::b_collect, &[1]),
    c!("coeffs", 1, 2, sy::b_coeffs, &[1]),
    c!("degree", 1, 2, sy::b_degree, &[1]),
    c!("partfrac", 1, 2, sy::b_partfrac, &[1]),
    c!("diff", 1, VAR, sy::b_diff, &[1, 2, 3, 4]),
    c!("solve", 1, 4, sy::b_solve, &[1]),
    c!("nsolve", 2, 4, sy::b_nsolve, &[1]),
    c!("roots", 1, 2, sy::b_roots, &[1]),
    c!("subs", 2, 3, sy::b_subs, &[1]),
    c!("steps", 1, 1, sy::b_steps, &[]),
];

/// Alternative names: `alias → canonical`.
pub static ALIASES: &[(&str, &str)] = &[
    ("lg", "log"),
    ("approx", "num"),
    ("arcsin", "asin"),
    ("arccos", "acos"),
    ("arctan", "atan"),
    ("arcsec", "asec"),
    ("arccsc", "acsc"),
    ("arccot", "acot"),
    ("arsinh", "asinh"),
    ("arcsinh", "asinh"),
    ("arcosh", "acosh"),
    ("arccosh", "acosh"),
    ("artanh", "atanh"),
    ("arctanh", "atanh"),
];

struct Table {
    by_name: HashMap<&'static str, (&'static Builtin, Arc<Function>)>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| {
    let mut by_name = HashMap::new();
    for b in BUILTINS {
        by_name.insert(b.name, (b, Arc::new(Function::Builtin(b))));
    }
    for (alias, target) in ALIASES {
        let entry = by_name.get(target).cloned().expect("alias target exists");
        by_name.insert(*alias, entry);
    }
    Table { by_name }
});

/// The builtin called `name` (aliases included).
pub fn lookup(name: &str) -> Option<&'static Builtin> {
    TABLE.by_name.get(name).map(|(b, _)| *b)
}

/// The builtin `name` as a function value.
pub fn function_value(name: &str) -> Option<Value> {
    TABLE
        .by_name
        .get(name)
        .map(|(_, f)| Value::Function(f.clone()))
}

fn first(args: Vec<Value>) -> Value {
    args.into_iter().next().expect("arity checked")
}

fn two(args: Vec<Value>) -> (Value, Value) {
    let mut it = args.into_iter();
    (it.next().expect("arity"), it.next().expect("arity"))
}

/// One-argument numeric builtins: `num1!(name, |n, prec| expr)`.
macro_rules! num1 {
    ($fname:ident, |$n:ident, $p:ident| $body:expr) => {
        fn $fname(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
            let $p = ev.opts.precision;
            ev.map_num(&first(args), span, &|$n: &Number| $body)
        }
    };
}

num1!(b_sqrt, |n, p| n.sqrt(p));
num1!(b_cbrt, |n, p| n.nth_root(&Number::from(3), p));
num1!(b_abs, |n, p| Ok(n.abs_prec(p)));
num1!(b_sign, |n, p| n.sign_prec(p));
num1!(b_exp, |n, p| n.exp(p));
num1!(b_ln, |n, p| n.ln(p));
num1!(b_sinh, |n, p| n.sinh(p));
num1!(b_cosh, |n, p| n.cosh(p));
num1!(b_tanh, |n, p| n.tanh(p));
num1!(b_csch, |n, p| n.sinh(p)?.recip());
num1!(b_sech, |n, p| n.cosh(p)?.recip());
num1!(b_coth, |n, p| n.tanh(p)?.recip());
num1!(b_asinh, |n, p| n.asinh(p));
num1!(b_acosh, |n, p| n.acosh(p));
num1!(b_atanh, |n, p| n.atanh(p));
num1!(b_floor, |n, _p| n.floor());
num1!(b_ceil, |n, _p| n.ceil());
num1!(b_trunc, |n, _p| n.trunc());
num1!(b_frac, |n, _p| Ok(n - &n.floor()?));
num1!(b_fact, |n, p| n.factorial(p));
num1!(b_gamma, |n, p| n.gamma(p));
num1!(b_re, |n, _p| Ok(n.re()));
num1!(b_im, |n, _p| Ok(n.im()));
num1!(b_conj, |n, _p| Ok(n.conj()));

fn b_root(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let p = ev.opts.precision;
    let (x, n) = two(args);
    ev.zip_num(&x, &n, span, &|x, n| x.nth_root(n, p))
}

fn b_log(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let p = ev.opts.precision;
    if args.len() == 1 {
        return ev.map_num(&first(args), span, &|n| n.log10(p));
    }
    let (x, b) = two(args);
    ev.zip_num(&x, &b, span, &|x, b| x.log(b, p))
}

fn b_binom(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (n, k) = two(args);
    ev.zip_num(&n, &k, span, &|n, k| n.ncr(k))
}

fn b_perm(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let (n, k) = two(args);
    ev.zip_num(&n, &k, span, &|n, k| n.npr(k))
}

// ----- trigonometry -----------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trig {
    Sin,
    Cos,
    Tan,
    Sec,
    Csc,
    Cot,
}

/// Exact values of trig functions at multiples of 30° and 45° whose result
/// is rational (degree mode only).
fn exact_degrees(t: Trig, x: &Number) -> Option<NumResult<Number>> {
    let r = x.to_rational()?;
    let int = |q: &BigRational, m: i64| -> Option<i64> {
        let k = q / BigRational::from_integer(BigInt::from(m));
        k.is_integer()
            .then(|| k.to_integer().mod_floor_i64(360 / m))?
    };
    let half = || Number::rational(1, 2).expect("nonzero");
    let sin30 = |m: i64| -> Option<Number> {
        Some(match m {
            0 | 6 => Number::zero(),
            1 | 5 => half(),
            7 | 11 => -half(),
            3 => Number::one(),
            9 => Number::from(-1),
            _ => return None,
        })
    };
    let ninety = BigRational::from_integer(BigInt::from(90));
    let value = match t {
        Trig::Sin | Trig::Csc => sin30(int(&r, 30)?)?,
        Trig::Cos | Trig::Sec => sin30(int(&(&r + &ninety), 30)?)?,
        Trig::Tan | Trig::Cot => {
            let v = match int(&r, 45)? {
                0 | 4 => Number::zero(),
                1 | 5 => Number::one(),
                3 | 7 => Number::from(-1),
                _ => {
                    // tan 90° is undefined, cot 90° = 0
                    return Some(if t == Trig::Cot {
                        Ok(Number::zero())
                    } else {
                        Err(NumberError::DivisionByZero)
                    });
                }
            };
            if t == Trig::Cot {
                return Some(v.recip());
            }
            v
        }
    };
    Some(match t {
        Trig::Sec | Trig::Csc => value.recip(),
        _ => Ok(value),
    })
}

type NumResult<T> = Result<T, NumberError>;

trait ModFloorI64 {
    fn mod_floor_i64(&self, m: i64) -> Option<i64>;
}

impl ModFloorI64 for BigInt {
    fn mod_floor_i64(&self, m: i64) -> Option<i64> {
        let r = num_integer::Integer::mod_floor(self, &BigInt::from(m));
        r.to_i64()
    }
}

fn trig_value(t: Trig, x: &Number, deg: Option<&Number>, p: Precision) -> NumResult<Number> {
    if let Some(f) = deg {
        if let Some(v) = exact_degrees(t, x) {
            return v;
        }
        let rad = x * f;
        return trig_value(t, &rad, None, p);
    }
    match t {
        Trig::Sin => x.sin(p),
        Trig::Cos => x.cos(p),
        Trig::Tan => x.tan(p),
        Trig::Sec => x.cos(p)?.recip(),
        Trig::Csc => x.sin(p)?.recip(),
        Trig::Cot => x.cos(p)?.checked_div(&x.sin(p)?),
    }
}

impl Evaluator<'_> {
    /// `π/180` in degree mode.
    fn degree_factor(&mut self) -> EvalResult<Option<Number>> {
        if self.opts.angle == AngleUnit::Degrees {
            let pi = self.pi()?;
            Ok(Some(
                pi.checked_div(&Number::from(180)).at(Span::default())?,
            ))
        } else {
            Ok(None)
        }
    }

    fn trig(&mut self, t: Trig, args: Vec<Value>, span: Span) -> EvalResult<Value> {
        let p = self.opts.precision;
        let deg = self.degree_factor()?;
        self.map_num(&first(args), span, &|x| trig_value(t, x, deg.as_ref(), p))
    }

    /// Inverse trig: result converted to degrees in degree mode.
    fn inverse_trig(
        &mut self,
        args: Vec<Value>,
        span: Span,
        f: &dyn Fn(&Number, Precision) -> NumResult<Number>,
    ) -> EvalResult<Value> {
        let p = self.opts.precision;
        let deg = self.degree_factor()?;
        self.map_num(&first(args), span, &|x| {
            let r = f(x, p)?;
            match &deg {
                Some(d) => r.checked_div(d),
                None => Ok(r),
            }
        })
    }

    fn half_pi(&mut self) -> EvalResult<Number> {
        let pi = self.pi()?;
        pi.checked_div(&Number::from(2)).at(Span::default())
    }
}

fn b_sin(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.trig(Trig::Sin, a, s)
}
fn b_cos(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.trig(Trig::Cos, a, s)
}
fn b_tan(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.trig(Trig::Tan, a, s)
}
fn b_sec(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.trig(Trig::Sec, a, s)
}
fn b_csc(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.trig(Trig::Csc, a, s)
}
fn b_cot(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.trig(Trig::Cot, a, s)
}
fn b_asin(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.inverse_trig(a, s, &|x, p| x.asin(p))
}
fn b_acos(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.inverse_trig(a, s, &|x, p| x.acos(p))
}
fn b_atan(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.inverse_trig(a, s, &|x, p| x.atan(p))
}
fn b_asec(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.inverse_trig(a, s, &|x, p| x.recip()?.acos(p))
}
fn b_acsc(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    ev.inverse_trig(a, s, &|x, p| x.recip()?.asin(p))
}
/// `acot(x) = π/2 − atan(x)`, continuous with range `(0, π)`.
fn b_acot(ev: &mut Evaluator<'_>, a: Vec<Value>, s: Span) -> EvalResult<Value> {
    let hp = ev.half_pi()?;
    ev.inverse_trig(a, s, &|x, p| Ok(&hp - &x.atan(p)?))
}

fn b_atan2(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let p = ev.opts.precision;
    let deg = ev.degree_factor()?;
    let (y, x) = two(args);
    ev.zip_num(&y, &x, span, &|y, x| {
        let r = Number::atan2(y, x, p)?;
        match &deg {
            Some(d) => r.checked_div(d),
            None => Ok(r),
        }
    })
}

fn b_arg(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let p = ev.opts.precision;
    let deg = ev.degree_factor()?;
    ev.map_num(&first(args), span, &|z| {
        let r = Number::atan2(&z.im(), &z.re(), p)?;
        match &deg {
            Some(d) => r.checked_div(d),
            None => Ok(r),
        }
    })
}

// ----- rounding and integers --------------------------------------------

fn b_round(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let p = ev.opts.precision;
    if args.len() == 1 {
        return ev.map_num(&first(args), span, &|n| n.round());
    }
    let (x, d) = two(args);
    ev.zip_num(&x, &d, span, &|x, d| {
        let d = d.integer_value()?;
        let scale = Number::from(10).pow(&Number::Integer(d), p)?;
        let r = (x * &scale).round()?.checked_div(&scale)?;
        Ok(match x {
            Number::F64(_) => r.into_f64_kind(),
            x if !x.is_exact() => r.to_float(p),
            _ => r,
        })
    })
}

/// Numbers from `gcd(a, b, ...)` or `gcd([a, b, ...])`.
fn number_args(ev: &Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Vec<Number>> {
    let items = match args.as_slice() {
        [Value::List(_)] => match first(args) {
            Value::List(l) => l,
            _ => unreachable!(),
        },
        _ => args,
    };
    items.into_iter().map(|v| ev.expect_num(v, span)).collect()
}

fn fold_numbers(
    ev: &mut Evaluator<'_>,
    args: Vec<Value>,
    span: Span,
    f: fn(&Number, &Number) -> NumResult<Number>,
) -> EvalResult<Value> {
    let nums = number_args(ev, args, span)?;
    let mut it = nums.into_iter();
    let Some(mut acc) = it.next() else {
        return Err(EvalError::number(NumberError::Domain("empty list"), span));
    };
    for n in it {
        acc = f(&acc, &n).at(span)?;
    }
    Ok(Value::Number(acc))
}

fn b_gcd(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    fold_numbers(ev, args, span, |a, b| a.gcd(b))
}

fn b_lcm(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    fold_numbers(ev, args, span, |a, b| a.lcm(b))
}

fn b_min(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    fold_numbers(ev, args, span, |a, b| a.min_real(b))
}

fn b_max(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    fold_numbers(ev, args, span, |a, b| a.max_real(b))
}

fn b_isprime(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    ev.map_value(first(args), span, &mut |ev, v| {
        let n = ev.expect_num(v, span)?;
        Ok(Value::Bool(n.is_prime().at(span)?))
    })
}

// ----- lists ------------------------------------------------------------

fn list_arg(v: Value, name: &str, span: Span) -> EvalResult<Vec<Value>> {
    match v {
        Value::List(items) | Value::Tuple(items) => Ok(items),
        Value::Matrix(m) => Ok(m.to_rows()),
        other => Err(EvalError::ty(
            format!("`{name}` needs a list, got a {}", other.type_name()),
            span,
        )),
    }
}

fn fold_list(
    ev: &mut Evaluator<'_>,
    args: Vec<Value>,
    span: Span,
    op: Arith,
    empty: Number,
    name: &str,
) -> EvalResult<Value> {
    let v = first(args);
    if let Value::Number(_) = v {
        return Ok(v);
    }
    let items = list_arg(v, name, span)?;
    let mut it = items.into_iter();
    let Some(mut acc) = it.next() else {
        return Ok(Value::Number(ev.mode_num(empty)));
    };
    for x in it {
        acc = ev.arith(op, acc, x, span)?;
    }
    Ok(acc)
}

fn b_sum(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    fold_list(ev, args, span, Arith::Add, Number::zero(), "sum")
}

fn b_prod(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    fold_list(ev, args, span, Arith::Mul, Number::one(), "prod")
}

fn b_len(_ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let n = match first(args) {
        Value::List(items) | Value::Tuple(items) => items.len(),
        Value::Matrix(m) => m.rows,
        Value::Str(s) => s.chars().count(),
        other => {
            return Err(EvalError::ty(
                format!("`len` needs a list, got a {}", other.type_name()),
                span,
            ));
        }
    };
    Ok(Value::num(Number::from(n as u64)))
}

fn b_sort(_ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let mut items = list_arg(first(args), "sort", span)?;
    if items.iter().all(|v| matches!(v, Value::Str(_))) {
        items.sort_by(|a, b| match (a, b) {
            (Value::Str(a), Value::Str(b)) => a.cmp(b),
            _ => unreachable!(),
        });
        return Ok(Value::List(items));
    }
    let mut nums = Vec::with_capacity(items.len());
    for v in items {
        match v {
            Value::Number(n) if n.is_real() => nums.push(n),
            other => {
                return Err(EvalError::ty(
                    format!(
                        "`sort` needs real numbers or strings, got a {}",
                        other.type_name()
                    ),
                    span,
                ));
            }
        }
    }
    nums.sort_by(|a, b| a.cmp_real(b).expect("real numbers"));
    Ok(Value::List(nums.into_iter().map(Value::Number).collect()))
}

fn b_reverse(_ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    match first(args) {
        Value::List(mut items) => {
            items.reverse();
            Ok(Value::List(items))
        }
        Value::Tuple(mut items) => {
            items.reverse();
            Ok(Value::Tuple(items))
        }
        Value::Str(s) => Ok(Value::Str(s.chars().rev().collect())),
        other => Err(EvalError::ty(
            format!("`reverse` needs a list, got a {}", other.type_name()),
            span,
        )),
    }
}

fn b_range(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let mut it = args.into_iter();
    let a = ev.expect_num(it.next().expect("arity"), span)?;
    let b = ev.expect_num(it.next().expect("arity"), span)?;
    let step = match it.next() {
        Some(s) => Some(ev.expect_num(s, span)?),
        None => None,
    };
    ev.range(&a, &b, step, span)
}

// ----- conversion -------------------------------------------------------

fn b_num(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let p = ev.opts.precision;
    let f64_mode = ev.opts.float64;
    let v = first(args);
    if let Value::Expr(s) = &v
        && !s.sym().is_constant()
    {
        return Ok(Value::sym(crate::cas::evalf(s.sym(), p)));
    }
    ev.map_num(&v, span, &|n| {
        Ok(if f64_mode {
            n.clone().into_f64_kind()
        } else {
            n.to_float(p)
        })
    })
}

/// Rationalises floats: the simplest fraction within the float's
/// precision (`exact(0.75) = 3/4`, `exact(1/3.0) = 1/3`). Values that need
/// a denominator above 10^15 are returned unchanged.
fn b_exact(ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    let v = first(args);
    if let Value::Expr(_) = v {
        return Ok(v);
    }
    ev.map_num(&v, span, &|n| Ok(rationalize(n)))
}

pub(crate) fn rationalize(n: &Number) -> Number {
    match n {
        Number::Complex(c) => Number::complex(rationalize(c.re()), rationalize(c.im())),
        Number::Integer(_) | Number::Rational(_) => n.clone(),
        Number::F64(x) => {
            let Some(f) = crate::number::BigFloat::from_f64(*x, 53) else {
                return n.clone();
            };
            simplest_rational(&f.to_rational(), 45).unwrap_or_else(|| n.clone())
        }
        Number::Float(f) => {
            let bits = f.prec().saturating_sub(8).max(8);
            simplest_rational(&f.to_rational(), bits).unwrap_or_else(|| n.clone())
        }
    }
}

/// Continued-fraction convergent of `x` within relative error `2^-bits`.
fn simplest_rational(x: &BigRational, bits: u64) -> Option<Number> {
    if x.is_zero() {
        return Some(Number::zero());
    }
    let tol = x.abs() / BigRational::from_integer(BigInt::one() << bits);
    let max_den = BigInt::from(10u64.pow(15));
    let (mut h0, mut h1) = (BigInt::zero(), BigInt::one());
    let (mut k0, mut k1) = (BigInt::one(), BigInt::zero());
    let mut rest = x.clone();
    for _ in 0..200 {
        let a = rest.floor().to_integer();
        let h2 = &a * &h1 + &h0;
        let k2 = &a * &k1 + &k0;
        if k2 > max_den {
            return None;
        }
        let approx = BigRational::new(h2.clone(), k2.clone());
        if (&approx - x).abs() <= tol {
            return Some(Number::from_ratio(approx));
        }
        let frac = &rest - BigRational::from_integer(a);
        if frac.is_zero() {
            return Some(Number::from_ratio(approx));
        }
        rest = frac.recip();
        (h0, h1, k0, k1) = (h1, h2, k1, k2);
    }
    None
}

// ----- matrices ---------------------------------------------------------

fn b_transpose(_ev: &mut Evaluator<'_>, args: Vec<Value>, span: Span) -> EvalResult<Value> {
    match first(args) {
        Value::Matrix(m) => Ok(Value::Matrix(m.transpose())),
        Value::List(items) => {
            let mut data = Vec::with_capacity(items.len());
            for v in items {
                match v {
                    Value::Number(n) => data.push(n),
                    other => {
                        return Err(EvalError::ty(
                            format!("`transpose` needs numbers, got a {}", other.type_name()),
                            span,
                        ));
                    }
                }
            }
            Ok(Value::Matrix(Matrix {
                rows: data.len(),
                cols: 1,
                data,
            }))
        }
        other => Err(EvalError::new(
            ErrorKind::Type,
            format!("`transpose` needs a matrix, got a {}", other.type_name()),
            span,
        )),
    }
}
