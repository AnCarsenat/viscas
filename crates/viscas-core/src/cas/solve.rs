//! Equation, system and inequality solving.
//!
//! - **Polynomials** with rational coefficients: factorization, then exact
//!   roots of each factor (linear, quadratic with surds/complex, cubic by
//!   Cardano or the trigonometric form, biquadratic quartics); other
//!   irreducible factors get numeric roots. Symbolic coefficients: degree
//!   1 and 2 (and biquadratic) formulas.
//! - **Rational equations**: the numerator's zeros, minus the zeros of the
//!   denominator (every candidate is checked by substitution).
//! - **Transcendental** equations by inversion (`sin`, `cos`, `tan`,
//!   `exp`, `ln`, powers, `|…|`), by the zero-product rule and by
//!   substitution of a kernel (`e^(2x) − 3e^x + 2 = 0` with `t = e^x`).
//!   Periodic solutions carry an integer parameter `k`
//!   (`π/6 + 2kπ`); with a domain the concrete solutions in it are listed.
//! - **Systems**: linear (exact Gaussian elimination, parametric when
//!   underdetermined) and small polynomial systems (substitution,
//!   resultants).
//! - **Inequalities** over the reals: a sign chart on the zeros, poles and
//!   domain boundaries, returning intervals.
//! - **Numeric**: [`nsolve`] (Newton from a guess, Brent on every sign
//!   change of an interval) and [`roots`] (all roots of a polynomial).

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{FromPrimitive, Signed, ToPrimitive, Zero};

use super::algebra::{cancel, expand, factor, numer_denom};
use super::approx::{approx, approx_f64, eval_f64};
use super::diff::diff;
use super::expr::{Func, Node, Sym, work_prec};
use super::poly::{Poly, is_poly_in, sym_coeffs};
use super::simplify::simplify;
use super::steps::record;
use crate::number::Number;
use crate::syntax::CmpOp;

/// Why solving failed.
#[derive(Clone, Debug, PartialEq)]
pub struct SolveError(pub String);

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type R<T> = Result<T, SolveError>;

fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(SolveError(msg.into()))
}

/// Options of [`solve`].
#[derive(Clone, Debug, Default)]
pub struct SolveOptions {
    /// Keep complex solutions (complex mode).
    pub complex: bool,
    /// Only solutions in `[lo, hi]` (expands periodic families).
    pub domain: Option<(f64, f64)>,
}

/// Result of [`solve`] for one unknown.
#[derive(Clone, Debug, PartialEq)]
pub enum Solution {
    /// Finitely many solutions (or periodic families with parameter `k`).
    Values(Vec<Sym>),
    /// Every value is a solution.
    All,
    /// Solution set of an inequality as relations (`x < -2`,
    /// `-2 < x ≤ 3`, `x = 1`).
    Intervals(Vec<Sym>),
}

const MAX_DEPTH: u32 = 10;

struct Ctx {
    complex: bool,
    param: String,
}

/// `lhs − rhs` of an equation, or the expression itself (`= 0`).
fn as_zero_form(eq: &Sym) -> R<Sym> {
    match eq.node() {
        Node::Rel(items, ops) => {
            if ops.len() != 1 || ops[0] != CmpOp::Eq {
                return fail("expected an equation");
            }
            Ok(&items[0] - &items[1])
        }
        _ => Ok(eq.clone()),
    }
}

fn param_name(e: &Sym) -> String {
    let vars = e.free_vars();
    if !vars.contains("k") {
        return "k".into();
    }
    (1..)
        .map(|i| format!("k_{i}"))
        .find(|n| !vars.contains(n))
        .expect("some name is free")
}

/// Solves an equation (`lhs = rhs` or an expression `= 0`) or an
/// inequality for `x`.
pub fn solve(eq: &Sym, x: &str, opts: &SolveOptions) -> R<Solution> {
    if let Node::Rel(_, ops) = eq.node()
        && ops.iter().any(|o| *o != CmpOp::Eq)
    {
        return solve_inequality(eq, x).map(|s| match s {
            Some(v) => Solution::Intervals(v),
            None => Solution::All,
        });
    }
    let f = as_zero_form(eq)?;
    record("move everything to one side", || {
        (eq.clone(), Sym::eq(f.clone(), Sym::zero()))
    });
    if !f.has_var(x) {
        let z = simplify(&f);
        return match super::approx::sign(&z) {
            Some(Ordering::Equal) => Ok(Solution::All),
            _ if z.is_exact_zero() => Ok(Solution::All),
            Some(_) => Ok(Solution::Values(Vec::new())),
            None => fail(format!("the equation does not contain `{x}`")),
        };
    }
    let ctx = Ctx {
        complex: opts.complex,
        param: param_name(&f),
    };
    let cands = solve_zero(&f, x, &ctx, 0)?;
    let sols = finalize(&f, x, cands, &ctx, opts.domain);
    record("check solutions", || {
        (
            Sym::eq(f.clone(), Sym::zero()),
            Sym::rel(
                vec![Sym::var(x), Sym::raw_add(sols.clone())],
                vec![CmpOp::Eq],
            ),
        )
    });
    Ok(Solution::Values(sols))
}

/// Filters (mode, domain, substitution check), deduplicates and sorts.
fn finalize(f: &Sym, x: &str, cands: Vec<Sym>, ctx: &Ctx, domain: Option<(f64, f64)>) -> Vec<Sym> {
    let mut out: Vec<Sym> = Vec::new();
    let push = |s: Sym, out: &mut Vec<Sym>| {
        if !out.iter().any(|o| same_value(o, &s)) {
            out.push(s);
        }
    };
    for c in cands {
        let c = tidy(&c);
        if c.has_undefined() {
            continue;
        }
        if !ctx.complex && !is_real(&c) {
            continue;
        }
        let periodic = c.has_var(&ctx.param);
        if let Some((lo, hi)) = domain {
            if periodic {
                for v in expand_family(&c, &ctx.param, lo, hi) {
                    if check(f, x, &v, ctx) {
                        push(v, &mut out);
                    }
                }
            } else if let Some(v) = approx_f64(&c)
                && v >= lo - 1e-12
                && v <= hi + 1e-12
                && check(f, x, &c, ctx)
            {
                push(c, &mut out);
            }
            continue;
        }
        let probe = if periodic {
            c.subs(&ctx.param, &Sym::zero())
        } else {
            c.clone()
        };
        if check(f, x, &probe, ctx) {
            push(c, &mut out);
        }
    }
    out.sort_by(|a, b| match (real_value(a), real_value(b)) {
        (Some(p), Some(q)) => p.partial_cmp(&q).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    out
}

fn real_value(s: &Sym) -> Option<f64> {
    if !s.is_constant() {
        return None;
    }
    let n = approx(s, work_prec()).ok()?;
    if n.is_real() { Some(n.to_f64()) } else { None }
}

/// Simplifies a candidate if that makes it smaller.
fn tidy(c: &Sym) -> Sym {
    if c.is_num() || c.as_var().is_some() || c.size() > 200 {
        return c.clone();
    }
    simplify(c)
}

fn same_value(a: &Sym, b: &Sym) -> bool {
    if a == b {
        return true;
    }
    if a.is_constant() && b.is_constant() {
        if let (Ok(p), Ok(q)) = (approx(a, work_prec()), approx(b, work_prec())) {
            let d = &p - &q;
            let (re, im) = d.to_complex_f64();
            let (pr, pi) = p.to_complex_f64();
            return re.hypot(im) <= 1e-12 * pr.hypot(pi).max(1.0);
        }
        return false;
    }
    (a - b).is_exact_zero() || cancel(&(a - b)).is_exact_zero()
}

/// Is a candidate real? Non-constant candidates count as real unless
/// they contain an explicit complex number.
fn is_real(c: &Sym) -> bool {
    if c.is_constant() {
        return approx(c, work_prec()).is_ok_and(|n| {
            if n.is_real() {
                return true;
            }
            let (re, im) = n.to_complex_f64();
            im.abs() <= 1e-25 * re.abs().max(1.0)
        });
    }
    !contains_complex(c)
}

fn contains_complex(c: &Sym) -> bool {
    match c.node() {
        Node::Num(n) => !n.is_real(),
        _ => c.children().iter().any(contains_complex),
    }
}

/// Does `x = v` satisfy `f = 0`? Undefined results fail; symbolic results
/// (free parameters) pass.
fn check(f: &Sym, x: &str, v: &Sym, ctx: &Ctx) -> bool {
    let s = f.subs(x, v);
    if s.has_undefined() {
        return false;
    }
    if !s.is_constant() {
        // Parameters left: accept unless it is clearly undefined.
        return true;
    }
    if s.is_exact_zero() {
        return true;
    }
    let Ok(val) = approx(&s, work_prec()) else {
        return false;
    };
    if !ctx.complex && !is_real(v) {
        return false;
    }
    if !ctx.complex && v.is_constant() {
        // Real mode: intermediate complex values (√ of negatives) fail.
        if !val.is_real() {
            let (re, im) = val.to_complex_f64();
            if im.abs() > 1e-20 * re.abs().max(1.0) {
                return false;
            }
        }
    }
    let scale = term_scale(&s);
    let (re, im) = val.to_complex_f64();
    re.hypot(im) <= 1e-12 * scale.max(1.0)
}

fn term_scale(s: &Sym) -> f64 {
    s.terms()
        .iter()
        .filter_map(|t| approx(t, work_prec()).ok())
        .map(|n| {
            let (a, b) = n.to_complex_f64();
            a.hypot(b)
        })
        .sum()
}

/// Members of a periodic family `c(k)` (linear in `k`) inside `[lo, hi]`.
fn expand_family(c: &Sym, k: &str, lo: f64, hi: f64) -> Vec<Sym> {
    let a = c.subs(k, &Sym::zero());
    let b = c.subs(k, &Sym::one());
    let (Some(a0), Some(a1)) = (approx_f64(&a), approx_f64(&b)) else {
        return Vec::new();
    };
    let step = a1 - a0;
    if step.abs() < 1e-300 {
        return Vec::new();
    }
    let (k1, k2) = {
        let p = (lo - a0) / step;
        let q = (hi - a0) / step;
        (p.min(q).ceil() as i64 - 1, p.max(q).floor() as i64 + 1)
    };
    if k2 - k1 > 2000 {
        return Vec::new();
    }
    (k1..=k2)
        .map(|kk| tidy(&c.subs(k, &Sym::int(kk))))
        .filter(|v| approx_f64(v).is_some_and(|t| t >= lo - 1e-12 && t <= hi + 1e-12))
        .collect()
}

// ----- univariate core ---------------------------------------------------------

/// Candidates for `f = 0` (not yet checked).
fn solve_zero(f: &Sym, x: &str, ctx: &Ctx, depth: u32) -> R<Vec<Sym>> {
    if depth > MAX_DEPTH {
        return fail("equation too deeply nested to solve");
    }
    if !f.has_var(x) {
        return Ok(Vec::new());
    }
    let (n, d) = numer_denom(f);
    let n = expand(&n);
    if !d.is_one() {
        record("multiply by the denominator", || {
            (
                Sym::eq(f.clone(), Sym::zero()),
                Sym::eq(n.clone(), Sym::zero()),
            )
        });
    }
    if !n.has_var(x) {
        return Ok(Vec::new());
    }
    if is_poly_in(&n, x)
        && let Some(c) = sym_coeffs(&n, x)
    {
        return poly_candidates(&n, &c, x, ctx);
    }
    // Zero product.
    let fz = factor(&n);
    let parts: Vec<Sym> = fz
        .factors()
        .into_iter()
        .map(|p| match p.node() {
            Node::Pow(b, e) if e.as_i64().is_some_and(|k| k > 0) => b.clone(),
            _ => p,
        })
        .filter(|p| p.has_var(x))
        .collect();
    if parts.len() >= 2 || (parts.len() == 1 && parts[0] != n) {
        record("zero product rule", || (n.clone(), fz.clone()));
        let mut out = Vec::new();
        for p in parts {
            out.extend(solve_zero(&p, x, ctx, depth + 1)?);
        }
        return Ok(out);
    }
    let norm = split_exp(&n, x);
    match isolate(&norm, &Sym::zero(), x, ctx, depth) {
        Ok(v) => Ok(v),
        Err(e) => {
            if let Some(v) = by_substitution(&norm, x, ctx, depth)? {
                return Ok(v);
            }
            if let Some(v) = by_raising(&n, x, ctx, depth)? {
                return Ok(v);
            }
            Err(e)
        }
    }
}

/// Candidates from a polynomial in `x` with coefficients `c` (low first).
fn poly_candidates(n: &Sym, c: &[Sym], x: &str, ctx: &Ctx) -> R<Vec<Sym>> {
    if let Some(p) = Poly::from_sym(n, x) {
        return Ok(poly_roots(&p, ctx.complex)
            .into_iter()
            .map(|(r, _)| r)
            .collect());
    }
    let deg = c.len() - 1;
    let z = Sym::zero();
    let get = |i: usize| c.get(i).cloned().unwrap_or_else(|| z.clone());
    match deg {
        0 => Ok(Vec::new()),
        1 => {
            let r = -get(0) / get(1);
            record("solve the linear equation", || (n.clone(), r.clone()));
            Ok(vec![r])
        }
        2 => Ok(quadratic(&get(2), &get(1), &get(0))),
        4 if get(1).is_exact_zero() && get(3).is_exact_zero() => {
            let mut out = Vec::new();
            for y in quadratic(&get(4), &get(2), &get(0)) {
                let r = y.sqrt();
                out.push(-&r);
                out.push(r);
            }
            Ok(out)
        }
        _ => fail(format!(
            "cannot solve a degree {deg} polynomial with symbolic coefficients"
        )),
    }
}

/// `a x² + b x + c = 0`.
fn quadratic(a: &Sym, b: &Sym, c: &Sym) -> Vec<Sym> {
    let disc = expand(&(b.powi(2) - Sym::int(4) * a * c));
    let two_a = Sym::int(2) * a;
    if disc.is_exact_zero() {
        let r = -b / &two_a;
        record("quadratic formula (double root)", || {
            (a * Sym::var("x").powi(2) + b * Sym::var("x") + c, r.clone())
        });
        return vec![r];
    }
    let s = disc.sqrt();
    let r1 = cancel(&((-b - &s) / &two_a));
    let r2 = cancel(&((-b + &s) / &two_a));
    let r1 = if r1.size() <= ((-b - &s) / &two_a).size() {
        r1
    } else {
        (-b - &s) / &two_a
    };
    let r2 = if r2.size() <= ((-b + &s) / &two_a).size() {
        r2
    } else {
        (-b + &s) / &two_a
    };
    record("quadratic formula", || {
        (
            Sym::rel(vec![Sym::var("Δ"), disc.clone()], vec![CmpOp::Eq]),
            Sym::raw_add(vec![r1.clone(), r2.clone()]),
        )
    });
    vec![r1, r2]
}

/// Roots of a rational polynomial with multiplicities: exact where
/// possible, numeric otherwise. Complex roots are included only if
/// `complex` (they are always computed for numeric factors and dropped).
pub fn poly_roots(p: &Poly, complex: bool) -> Vec<(Sym, usize)> {
    let mut out = Vec::new();
    if p.deg() == 0 {
        return out;
    }
    let (_, facs) = p.factor();
    if facs.len() > 1 || facs.first().is_some_and(|f| f.1 > 1) {
        record("factor", || {
            let x = Sym::var("x");
            (
                p.to_sym(&x),
                Sym::raw_mul(
                    facs.iter()
                        .map(|(f, m)| {
                            if *m == 1 {
                                f.to_sym(&x)
                            } else {
                                Sym::raw_pow(f.to_sym(&x), Sym::int(*m as i64))
                            }
                        })
                        .collect(),
                ),
            )
        });
    }
    for (f, m) in facs {
        for r in irreducible_roots(&f, complex) {
            out.push((r, m));
        }
    }
    out
}

fn rq(r: &BigRational) -> Sym {
    Sym::ratio(r.clone())
}

fn irreducible_roots(f: &Poly, complex: bool) -> Vec<Sym> {
    let c = &f.c;
    match f.deg() {
        1 => vec![rq(&(-&c[0] / &c[1]))],
        2 => quadratic(&rq(&c[2]), &rq(&c[1]), &rq(&c[0])),
        3 => cubic(&c[3], &c[2], &c[1], &c[0], complex),
        4 if c[1].is_zero() && c[3].is_zero() => {
            let mut out = Vec::new();
            for y in quadratic(&rq(&c[4]), &rq(&c[2]), &rq(&c[0])) {
                let r = y.sqrt();
                out.push(-&r);
                out.push(r);
            }
            out
        }
        _ => numeric_roots(f, complex),
    }
}

fn numeric_roots(f: &Poly, complex: bool) -> Vec<Sym> {
    let roots = f.numeric_roots();
    record("numeric roots (Aberth)", || {
        (
            f.to_sym(&Sym::var("x")),
            Sym::raw_add(
                roots
                    .iter()
                    .filter_map(|&(re, im)| Number::f64(re).ok().zip(Number::f64(im).ok()))
                    .map(|(a, b)| Sym::num(Number::complex(a, b)))
                    .collect(),
            ),
        )
    });
    roots
        .into_iter()
        .filter(|&(_, im)| complex || im == 0.0)
        .filter_map(|(re, im)| {
            let n = Number::complex(Number::f64(re).ok()?, Number::f64(im).ok()?);
            Some(Sym::num(n))
        })
        .collect()
}

/// Roots of an irreducible cubic: Cardano (one real root) or the
/// trigonometric form (three real roots).
fn cubic(
    a: &BigRational,
    b: &BigRational,
    c: &BigRational,
    d: &BigRational,
    complex: bool,
) -> Vec<Sym> {
    let three = BigRational::from_integer(BigInt::from(3));
    let p = (&three * a * c - b * b) / (&three * a * a);
    let q = (BigRational::from_integer(BigInt::from(2)) * b * b * b
        - BigRational::from_integer(BigInt::from(9)) * a * b * c
        + BigRational::from_integer(BigInt::from(27)) * a * a * d)
        / (BigRational::from_integer(BigInt::from(27)) * a * a * a);
    let shift = rq(&(-b / (&three * a)));
    let disc = &q * &q / BigRational::from_integer(BigInt::from(4))
        + &p * &p * &p / BigRational::from_integer(BigInt::from(27));
    let half_q = rq(&(-&q / BigRational::from_integer(BigInt::from(2))));
    let third = Sym::rat(1, 3);
    if disc.is_positive() {
        let sd = rq(&disc).sqrt();
        let u = Sym::pow(&half_q + &sd, third.clone());
        let v = Sym::pow(&half_q - &sd, third);
        let real = &u + &v + &shift;
        record("Cardano's formula", || (rq(&disc), real.clone()));
        let mut out = vec![real];
        if complex {
            let w = Sym::rat(-1, 2) + Sym::i() * Sym::int(3).sqrt() / Sym::int(2);
            let wb = Sym::rat(-1, 2) - Sym::i() * Sym::int(3).sqrt() / Sym::int(2);
            out.push(&w * &u + &wb * &v + &shift);
            out.push(&wb * &u + &w * &v + &shift);
        }
        out
    } else {
        // Three real roots: 2√(−p/3)·cos(θ/3 − 2πk/3).
        let m = Sym::int(2) * rq(&(-&p / &three)).sqrt();
        let arg = rq(&(&three * &q / (BigRational::from_integer(BigInt::from(2)) * &p)))
            * rq(&(-&three / &p)).sqrt();
        let theta = Sym::rat(1, 3) * arg.apply(Func::Acos);
        let roots: Vec<Sym> = (0..3)
            .map(|k| &m * (&theta - Sym::rat(2 * k, 3) * Sym::pi()).apply(Func::Cos) + &shift)
            .collect();
        record("trigonometric form of the cubic", || {
            (rq(&disc), Sym::raw_add(roots.clone()))
        });
        roots
    }
}

/// Rewrites `b^(k·u + c)` (b constant) as `(b^u)^k · b^c` so that
/// kernels like `e^x` appear explicitly.
fn split_exp(e: &Sym, x: &str) -> Sym {
    if !e.has_var(x) {
        return e.clone();
    }
    match e.node() {
        Node::Pow(b, h) if !b.has_var(x) && h.has_var(x) => {
            let mut pieces = Vec::new();
            for t in h.terms() {
                let (c, rest) = t.coeff_rest();
                if rest.has_var(x)
                    && let Some(k) = c.to_rational().filter(|r| r.is_integer() && c.is_exact())
                    && let Some(k) = k.to_integer().to_i64()
                    && k.abs() >= 2
                    && k.abs() <= 12
                {
                    pieces.push(Sym::raw_pow(Sym::pow(b.clone(), rest), Sym::int(k)));
                } else {
                    pieces.push(Sym::pow(b.clone(), t));
                }
            }
            Sym::raw_mul(pieces)
        }
        Node::Add(ts) => Sym::raw_add(ts.iter().map(|t| split_exp(t, x)).collect()),
        Node::Mul(fs) => Sym::raw_mul(fs.iter().map(|t| split_exp(t, x)).collect()),
        _ => e.clone(),
    }
}

/// Subexpressions that could serve as a substitution variable.
fn kernels(e: &Sym, x: &str, out: &mut Vec<Sym>) {
    if !e.has_var(x) {
        return;
    }
    match e.node() {
        Node::Fn(..) => {
            if !out.contains(e) {
                out.push(e.clone());
            }
        }
        Node::Pow(b, h) => {
            let integer_exp = h.as_integer().is_some();
            if !integer_exp && !out.contains(e) {
                out.push(e.clone());
            }
            kernels(b, x, out);
            kernels(h, x, out);
            return;
        }
        _ => {}
    }
    for c in e.children() {
        kernels(&c, x, out);
    }
}

fn by_substitution(n: &Sym, x: &str, ctx: &Ctx, depth: u32) -> R<Option<Vec<Sym>>> {
    let mut ks = Vec::new();
    kernels(n, x, &mut ks);
    ks.sort_by_key(|k| std::cmp::Reverse(k.size()));
    let t = Sym::var("_t");
    for k in ks {
        let g = n.replace(&k, &t);
        if g.has_var(x) || !is_poly_in(&expand(&g), "_t") {
            continue;
        }
        record("substitute t", || (n.clone(), g.clone()));
        let ts = solve_zero(&g, "_t", ctx, depth + 1)?;
        let mut out = Vec::new();
        for tv in ts {
            if !ctx.complex && !is_real(&tv) {
                continue;
            }
            if let Ok(v) = solve_zero(&(&k - &tv), x, ctx, depth + 1) {
                out.extend(v);
            }
        }
        return Ok(Some(out));
    }
    Ok(None)
}

/// One radical term `c·b^(p/q)` plus rest: isolate it and raise both
/// sides to the power q (extraneous roots are removed by the check).
fn by_raising(n: &Sym, x: &str, ctx: &Ctx, depth: u32) -> R<Option<Vec<Sym>>> {
    let terms = n.terms();
    for (i, t) in terms.iter().enumerate() {
        let (c, rest_t) = t.coeff_rest();
        let Node::Pow(b, e) = rest_t.node() else {
            continue;
        };
        let Some(r) = e.as_rational() else { continue };
        if r.is_integer() || !b.has_var(x) {
            continue;
        }
        let (Some(pn), Some(qd)) = (r.numer().to_i64(), r.denom().to_i64()) else {
            continue;
        };
        if qd > 6 || pn.abs() > 6 {
            continue;
        }
        let mut others = terms.clone();
        others.remove(i);
        let rhs = -Sym::add(others) / Sym::num(c);
        // b^(p/q) = rhs ⇒ b^p = rhs^q
        let g = expand(&(b.powi(pn) - rhs.powi(qd)));
        record("raise both sides to a power", || {
            (
                Sym::eq(rest_t.clone(), rhs.clone()),
                Sym::eq(g.clone(), Sym::zero()),
            )
        });
        if !g.has_var(x) {
            return Ok(Some(Vec::new()));
        }
        return solve_zero(&g, x, ctx, depth + 1).map(Some);
    }
    Ok(None)
}

/// Solves `lhs = rhs` by peeling off invertible operations.
fn isolate(lhs: &Sym, rhs: &Sym, x: &str, ctx: &Ctx, depth: u32) -> R<Vec<Sym>> {
    if depth > MAX_DEPTH {
        return fail("equation too deeply nested to solve");
    }
    if lhs.as_var() == Some(x) {
        return Ok(vec![rhs.clone()]);
    }
    let next = |l: &Sym, r: Sym, rule: &str| -> R<Vec<Sym>> {
        record(rule, || {
            (
                Sym::eq(lhs.clone(), rhs.clone()),
                Sym::eq(l.clone(), r.clone()),
            )
        });
        isolate(l, &r, x, ctx, depth + 1)
    };
    let k = Sym::var(&ctx.param);
    let two_pi_k = Sym::int(2) * Sym::pi() * &k;
    match lhs.node() {
        Node::Add(ts) => {
            let (with, without): (Vec<Sym>, Vec<Sym>) =
                ts.iter().cloned().partition(|t| t.has_var(x));
            if with.len() == 1 {
                return next(
                    &with[0],
                    rhs - Sym::add(without),
                    "subtract from both sides",
                );
            }
            fail(format!("cannot isolate `{x}`"))
        }
        Node::Mul(fs) => {
            let (with, without): (Vec<Sym>, Vec<Sym>) =
                fs.iter().cloned().partition(|t| t.has_var(x));
            if with.len() == 1 {
                return next(&with[0], rhs / Sym::mul(without), "divide both sides");
            }
            if rhs.is_exact_zero() {
                let mut out = Vec::new();
                for w in with {
                    out.extend(isolate(&w, rhs, x, ctx, depth + 1)?);
                }
                return Ok(out);
            }
            fail(format!("cannot isolate `{x}`"))
        }
        Node::Pow(b, n) if !n.has_var(x) => {
            if rhs.is_exact_zero() {
                return isolate(b, rhs, x, ctx, depth + 1);
            }
            if let Some(k) = n.as_i64() {
                let root = Sym::pow(rhs.clone(), Sym::rat(1, k));
                let mut out = Vec::new();
                if k % 2 == 0 {
                    out.extend(next(b, root.clone(), "take the root of both sides")?);
                    out.extend(next(b, -root, "take the root of both sides")?);
                } else {
                    out.extend(next(b, root, "take the root of both sides")?);
                }
                if ctx.complex && k.abs() > 2 {
                    // Other complex roots: ω^j·root.
                    for j in 1..k.abs() {
                        let w = (Sym::int(2 * j) * Sym::pi() * Sym::i() / Sym::int(k.abs())).exp();
                        let r = Sym::pow(rhs.clone(), Sym::rat(1, k)) * w;
                        if let Ok(v) = isolate(b, &r, x, ctx, depth + 1) {
                            out.extend(v);
                        }
                    }
                }
                return Ok(out);
            }
            next(
                b,
                Sym::pow(rhs.clone(), n.recip()),
                "raise both sides to the inverse power",
            )
        }
        Node::Pow(b, h) if !b.has_var(x) => {
            let l = if matches!(b.node(), Node::Const(super::expr::Const::E)) {
                rhs.ln()
            } else {
                rhs.ln() / b.ln()
            };
            next(h, l, "take the logarithm")
        }
        Node::Fn(f, u) => {
            let mut out = Vec::new();
            match f {
                Func::Ln => out.extend(next(u, rhs.exp(), "exponentiate")?),
                Func::Abs => {
                    if super::approx::sign(rhs) == Some(Ordering::Less) {
                        return Ok(Vec::new());
                    }
                    out.extend(next(u, rhs.clone(), "|u| = c ⇒ u = c")?);
                    out.extend(next(u, -rhs, "|u| = c ⇒ u = −c")?);
                }
                Func::Sin => {
                    let a = rhs.apply(Func::Asin);
                    out.extend(next(u, &a + &two_pi_k, "inverse sine (general solution)")?);
                    out.extend(next(
                        u,
                        Sym::pi() - a + &two_pi_k,
                        "inverse sine (general solution)",
                    )?);
                }
                Func::Cos => {
                    let a = rhs.apply(Func::Acos);
                    out.extend(next(
                        u,
                        &a + &two_pi_k,
                        "inverse cosine (general solution)",
                    )?);
                    out.extend(next(
                        u,
                        -a + &two_pi_k,
                        "inverse cosine (general solution)",
                    )?);
                }
                Func::Tan => {
                    let a = rhs.apply(Func::Atan) + Sym::pi() * &k;
                    out.extend(next(u, a, "inverse tangent (general solution)")?);
                }
                Func::Asin => out.extend(next(u, rhs.apply(Func::Sin), "apply sin")?),
                Func::Acos => out.extend(next(u, rhs.apply(Func::Cos), "apply cos")?),
                Func::Atan => out.extend(next(u, rhs.apply(Func::Tan), "apply tan")?),
                Func::Sinh => out.extend(next(u, rhs.apply(Func::Asinh), "apply asinh")?),
                Func::Cosh => {
                    let a = rhs.apply(Func::Acosh);
                    out.extend(next(u, a.clone(), "apply acosh")?);
                    out.extend(next(u, -a, "apply acosh")?);
                }
                Func::Tanh => out.extend(next(u, rhs.apply(Func::Atanh), "apply atanh")?),
                Func::Asinh => out.extend(next(u, rhs.apply(Func::Sinh), "apply sinh")?),
                Func::Acosh => out.extend(next(u, rhs.apply(Func::Cosh), "apply cosh")?),
                Func::Atanh => out.extend(next(u, rhs.apply(Func::Tanh), "apply tanh")?),
                _ => return fail(format!("cannot invert `{}`", f.name())),
            }
            Ok(out)
        }
        _ => fail(format!("cannot isolate `{x}`")),
    }
}

// ----- roots / nsolve --------------------------------------------------------------

/// All roots of a polynomial in `x` with multiplicity (exact where
/// possible). Complex roots are included when `complex`.
pub fn roots(e: &Sym, x: &str, complex: bool) -> R<Vec<Sym>> {
    let (n, _) = numer_denom(&cancel(e));
    let n = expand(&n);
    let Some(p) = Poly::from_sym(&n, x) else {
        if let Some(c) = sym_coeffs(&n, x) {
            let ctx = Ctx {
                complex,
                param: "k".into(),
            };
            return poly_candidates(&n, &c, x, &ctx);
        }
        return fail(format!("not a polynomial in `{x}`"));
    };
    let mut out = Vec::new();
    for (r, m) in poly_roots(&p, complex) {
        let r = tidy(&r);
        if !complex && !is_real(&r) {
            continue;
        }
        for _ in 0..m {
            out.push(r.clone());
        }
    }
    out.sort_by(|a, b| match (real_value(a), real_value(b)) {
        (Some(p), Some(q)) => p.partial_cmp(&q).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    Ok(out)
}

/// Starting information for [`nsolve`].
#[derive(Clone, Copy, Debug)]
pub enum NStart {
    Guess(f64),
    Interval(f64, f64),
}

/// Numeric roots of `f = 0`: one root from a guess (Newton, then secant),
/// or every root found in an interval (sign changes + Brent, and
/// near-tangent minima polished by Newton).
pub fn nsolve(eq: &Sym, x: &str, start: NStart) -> R<Vec<f64>> {
    let f = as_zero_form(eq)?;
    let free: Vec<String> = f.free_vars().into_iter().filter(|v| v != x).collect();
    if !free.is_empty() {
        return fail(format!("nsolve: unknown names {}", free.join(", ")));
    }
    let df = diff(&f, x).ok();
    let fx = |t: f64| eval_f64(&f, x, t);
    let dfx = |t: f64| match &df {
        Some(d) => eval_f64(d, x, t),
        None => {
            let h = 1e-6 * t.abs().max(1.0);
            (fx(t + h) - fx(t - h)) / (2.0 * h)
        }
    };
    let ok = |t: f64| {
        let v = fx(t);
        v.is_finite() && v.abs() <= 1e-9 * (1.0 + t.abs())
    };
    match start {
        NStart::Guess(g) => {
            let r = newton(&fx, &dfx, g).or_else(|| secant(&fx, g, g + 1e-4 * g.abs().max(1.0)));
            match r {
                Some(r) if ok(r) => Ok(vec![clean(r)]),
                _ => fail("nsolve: no convergence from this starting point"),
            }
        }
        NStart::Interval(a, b) => {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            let n = 2000;
            let h = (b - a) / n as f64;
            let mut out: Vec<f64> = Vec::new();
            let push = |r: f64, out: &mut Vec<f64>| {
                if r >= a - 1e-12
                    && r <= b + 1e-12
                    && !out.iter().any(|o| (o - r).abs() <= 1e-9 * (1.0 + r.abs()))
                {
                    out.push(clean(r));
                }
            };
            let xs: Vec<f64> = (0..=n).map(|i| a + h * i as f64).collect();
            let ys: Vec<f64> = xs.iter().map(|&t| fx(t)).collect();
            for i in 0..n {
                let (y0, y1) = (ys[i], ys[i + 1]);
                if y0 == 0.0 {
                    push(xs[i], &mut out);
                    continue;
                }
                if y0.is_finite() && y1.is_finite() && y0.signum() != y1.signum() && y1 != 0.0 {
                    if let Some(r) = brent(&fx, xs[i], xs[i + 1])
                        && ok(r)
                    {
                        push(r, &mut out);
                    }
                } else if i > 0
                    && y0.is_finite()
                    && ys[i - 1].is_finite()
                    && y1.is_finite()
                    && y0.abs() < ys[i - 1].abs()
                    && y0.abs() < y1.abs()
                    && y0.abs() < 1e-3
                {
                    // Possible double root: Newton from the local minimum.
                    if let Some(r) = newton(&fx, &dfx, xs[i])
                        && ok(r)
                    {
                        push(r, &mut out);
                    }
                }
            }
            if ys[n] == 0.0 {
                push(b, &mut out);
            }
            out.sort_by(|p, q| p.partial_cmp(q).unwrap_or(Ordering::Equal));
            Ok(out)
        }
    }
}

fn clean(r: f64) -> f64 {
    if r.abs() < 1e-14 { 0.0 } else { r }
}

fn newton(f: &dyn Fn(f64) -> f64, df: &dyn Fn(f64) -> f64, mut t: f64) -> Option<f64> {
    for _ in 0..100 {
        let y = f(t);
        if !y.is_finite() {
            return None;
        }
        if y == 0.0 {
            return Some(t);
        }
        let d = df(t);
        if !d.is_finite() || d == 0.0 {
            return None;
        }
        let step = y / d;
        t -= step;
        if step.abs() <= 1e-15 * t.abs().max(1.0) {
            return Some(t);
        }
    }
    let y = f(t);
    (y.is_finite() && y.abs() < 1e-10).then_some(t)
}

fn secant(f: &dyn Fn(f64) -> f64, mut a: f64, mut b: f64) -> Option<f64> {
    let (mut fa, mut fb) = (f(a), f(b));
    for _ in 0..200 {
        if !fa.is_finite() || !fb.is_finite() || fb == fa {
            break;
        }
        let c = b - fb * (b - a) / (fb - fa);
        a = b;
        fa = fb;
        b = c;
        fb = f(b);
        if (b - a).abs() <= 1e-15 * b.abs().max(1.0) {
            return Some(b);
        }
    }
    (fb.is_finite() && fb.abs() < 1e-10).then_some(b)
}

/// Brent's method on a bracketing interval.
fn brent(f: &dyn Fn(f64) -> f64, mut a: f64, mut b: f64) -> Option<f64> {
    let (mut fa, mut fb) = (f(a), f(b));
    if fa.signum() == fb.signum() {
        return None;
    }
    if fa.abs() < fb.abs() {
        std::mem::swap(&mut a, &mut b);
        std::mem::swap(&mut fa, &mut fb);
    }
    let (mut c, mut fc) = (a, fa);
    let mut mflag = true;
    let mut d = 0.0;
    for _ in 0..200 {
        if fb == 0.0 || (b - a).abs() <= 1e-15 * b.abs().max(1.0) {
            return Some(b);
        }
        let mut s = if fa != fc && fb != fc {
            a * fb * fc / ((fa - fb) * (fa - fc))
                + b * fa * fc / ((fb - fa) * (fb - fc))
                + c * fa * fb / ((fc - fa) * (fc - fb))
        } else {
            b - fb * (b - a) / (fb - fa)
        };
        let lo = (3.0 * a + b) / 4.0;
        let cond1 = !((s > lo.min(b)) && (s < lo.max(b)));
        let cond2 = mflag && (s - b).abs() >= (b - c).abs() / 2.0;
        let cond3 = !mflag && (s - b).abs() >= (c - d).abs() / 2.0;
        if cond1 || cond2 || cond3 {
            s = (a + b) / 2.0;
            mflag = true;
        } else {
            mflag = false;
        }
        let fs = f(s);
        d = c;
        c = b;
        fc = fb;
        if fa.signum() != fs.signum() {
            b = s;
            fb = fs;
        } else {
            a = s;
            fa = fs;
        }
        if fa.abs() < fb.abs() {
            std::mem::swap(&mut a, &mut b);
            std::mem::swap(&mut fa, &mut fb);
        }
    }
    Some(b)
}

// ----- inequalities --------------------------------------------------------------

/// Domain boundaries: zeros of even-root radicands and log arguments.
fn domain_points(e: &Sym, x: &str, out: &mut Vec<Sym>) {
    match e.node() {
        Node::Pow(b, h)
            if b.has_var(x)
                && h.as_rational()
                    .is_some_and(|r| (r.denom() % BigInt::from(2)).is_zero()) =>
        {
            out.push(b.clone());
        }
        Node::Fn(Func::Ln, a) if a.has_var(x) => out.push(a.clone()),
        Node::Fn(Func::Asin | Func::Acos, a) if a.has_var(x) => {
            out.push(a - Sym::one());
            out.push(a + Sym::one());
        }
        _ => {}
    }
    for c in e.children() {
        domain_points(&c, x, out);
    }
}

/// Real solution set of a (chained) inequality in `x`: `None` means all
/// reals.
pub fn solve_inequality(rel: &Sym, x: &str) -> R<Option<Vec<Sym>>> {
    let Node::Rel(items, ops) = rel.node() else {
        return fail("expected an inequality");
    };
    // A chain a < b < c is the intersection of its links: evaluate the
    // sign chart of every link together.
    let links: Vec<(Sym, CmpOp)> = ops
        .iter()
        .enumerate()
        .map(|(i, op)| (&items[i] - &items[i + 1], *op))
        .collect();
    let ctx = Ctx {
        complex: false,
        param: "k".into(),
    };
    let mut crit: Vec<(f64, Sym)> = Vec::new();
    for (f, _) in &links {
        if !f.has_var(x) {
            continue;
        }
        let (n, d) = numer_denom(&cancel(f));
        let mut zs = vec![n, d];
        domain_points(f, x, &mut zs);
        for z in zs {
            if !z.has_var(x) {
                continue;
            }
            let cands = solve_zero(&z, x, &ctx, 0)
                .map_err(|e| SolveError(format!("cannot solve the inequality: {}", e.0)))?;
            for c in cands {
                if c.has_var(&ctx.param) {
                    return fail("cannot solve the inequality: periodic boundary points");
                }
                let c = tidy(&c);
                let Some(v) = real_value(&c) else { continue };
                if !crit
                    .iter()
                    .any(|(w, _)| (w - v).abs() <= 1e-12 * v.abs().max(1.0))
                {
                    crit.push((v, c));
                }
            }
        }
    }
    crit.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
    let holds = |at: &Sym| -> bool {
        links.iter().all(|(f, op)| {
            let s = f.subs(x, at);
            if s.has_undefined() {
                return false;
            }
            let Ok(v) = approx(&s, work_prec()) else {
                return false;
            };
            if !v.is_real() {
                return false;
            }
            let t = v.to_f64();
            let zero = s.is_exact_zero() || t.abs() <= 1e-20 * term_scale(&s).max(1.0);
            match op {
                CmpOp::Lt => !zero && t < 0.0,
                CmpOp::Le => zero || t < 0.0,
                CmpOp::Gt => !zero && t > 0.0,
                CmpOp::Ge => zero || t > 0.0,
                CmpOp::Ne => !zero,
                CmpOp::Eq => zero,
            }
        })
    };
    let sample = |v: f64| -> Sym {
        BigRational::from_f64(v)
            .map(|r| {
                // A short rational near v keeps the substitution cheap.
                let approx = (v * 1024.0).round() / 1024.0;
                BigRational::from_f64(approx)
                    .filter(|_| approx != 0.0 || v == 0.0)
                    .unwrap_or(r)
            })
            .map(Sym::ratio)
            .unwrap_or_else(Sym::zero)
    };
    // Pieces: interval 0, point 1, interval 1, …
    let n = crit.len();
    let mut interval_ok = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let s = if n == 0 {
            Sym::zero()
        } else if i == 0 {
            sample(crit[0].0 - 1.0)
        } else if i == n {
            sample(crit[n - 1].0 + 1.0)
        } else {
            let (a, b) = (crit[i - 1].0, crit[i].0);
            let mid = (a + b) / 2.0;
            let s = sample(mid);
            let sv = approx_f64(&s).unwrap_or(mid);
            if sv > a && sv < b {
                s
            } else {
                Sym::num(Number::f64(mid).unwrap_or_else(|_| Number::zero()))
            }
        };
        interval_ok.push(holds(&s));
    }
    let point_ok: Vec<bool> = crit.iter().map(|(_, c)| holds(c)).collect();
    if interval_ok.iter().all(|&b| b) && point_ok.iter().all(|&b| b) {
        return Ok(None);
    }
    // Merge into maximal runs.
    let xv = Sym::var(x);
    let mut out = Vec::new();
    let mut i = 0;
    // Piece index p: even = interval p/2, odd = point (p-1)/2.
    let pieces = 2 * n + 1;
    let ok = |p: usize| {
        if p.is_multiple_of(2) {
            interval_ok[p / 2]
        } else {
            point_ok[p / 2]
        }
    };
    while i < pieces {
        if !ok(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i + 1 < pieces && ok(i + 1) {
            i += 1;
        }
        let end = i;
        i += 1;
        if start == end && start % 2 == 1 {
            out.push(Sym::eq(xv.clone(), crit[start / 2].1.clone()));
            continue;
        }
        let lo = if start == 0 {
            None
        } else if start % 2 == 1 {
            Some((crit[start / 2].1.clone(), CmpOp::Le))
        } else {
            Some((crit[start / 2 - 1].1.clone(), CmpOp::Lt))
        };
        let hi = if end == pieces - 1 {
            None
        } else if end % 2 == 1 {
            Some((crit[end / 2].1.clone(), CmpOp::Le))
        } else {
            Some((crit[end / 2].1.clone(), CmpOp::Lt))
        };
        out.push(match (lo, hi) {
            (None, None) => return Ok(None),
            (Some((a, op)), None) => Sym::rel(vec![xv.clone(), a], vec![flip(op)]),
            (None, Some((b, op))) => Sym::rel(vec![xv.clone(), b], vec![op]),
            (Some((a, o1)), Some((b, o2))) => Sym::rel(vec![a, xv.clone(), b], vec![o1, o2]),
        });
    }
    record("sign chart", || (rel.clone(), Sym::raw_add(out.clone())));
    Ok(Some(out))
}

fn flip(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Ge => CmpOp::Le,
        o => o,
    }
}

// ----- systems -----------------------------------------------------------------------

/// One solution of a system: `(variable, value)` pairs.
pub type Assignment = Vec<(String, Sym)>;

/// Solves a system of equations for `vars`. Returns every solution found
/// (an empty list if there is none). Underdetermined linear systems give
/// one parametric solution in terms of the free variables.
pub fn solve_system(eqs: &[Sym], vars: &[String], opts: &SolveOptions) -> R<Vec<Assignment>> {
    let mut fs = Vec::with_capacity(eqs.len());
    for e in eqs {
        let f = as_zero_form(e)?;
        let (n, _) = numer_denom(&cancel(&f));
        fs.push(expand(&n));
    }
    let ctx = Ctx {
        complex: opts.complex,
        param: "k".into(),
    };
    if let Some(sol) = linear_system(&fs, vars)? {
        return Ok(sol);
    }
    let sols = system_rec(fs.clone(), vars.to_vec(), &ctx, 0)?;
    // Verify and clean.
    let mut out: Vec<Assignment> = Vec::new();
    'outer: for s in sols {
        let s: Assignment = s.into_iter().map(|(v, e)| (v, tidy(&e))).collect();
        for (_, val) in &s {
            if val.has_undefined() || (!ctx.complex && !is_real(val)) {
                continue 'outer;
            }
        }
        for f in &fs {
            let mut g = f.clone();
            for (v, val) in &s {
                g = g.subs(v, val);
            }
            if g.is_constant() && !check(&g, "_none", &Sym::zero(), &ctx) {
                continue 'outer;
            }
        }
        if !out.iter().any(|o| {
            o.len() == s.len()
                && o.iter()
                    .zip(&s)
                    .all(|(a, b)| a.0 == b.0 && same_value(&a.1, &b.1))
        }) {
            out.push(s);
        }
    }
    out.sort_by(|a, b| {
        let key = |s: &Assignment| {
            s.iter()
                .map(|(_, v)| real_value(v).unwrap_or(f64::NAN))
                .collect::<Vec<_>>()
        };
        key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal)
    });
    Ok(out)
}

/// Linear coefficients of `f` in `vars`: `(a_j, b)` with `f = Σ a_j v_j + b`.
fn linear_coeffs(f: &Sym, vars: &[String]) -> Option<(Vec<Sym>, Sym)> {
    let mut a = vec![Vec::new(); vars.len()];
    let mut b = Vec::new();
    for t in f.terms() {
        let hits: Vec<usize> = (0..vars.len()).filter(|&j| t.has_var(&vars[j])).collect();
        match hits.len() {
            0 => b.push(t),
            1 => {
                let j = hits[0];
                let (k, c) = super::poly::term_degree(&t, &vars[j])?;
                if k != 1 {
                    return None;
                }
                a[j].push(c);
            }
            _ => return None,
        }
    }
    Some((a.into_iter().map(Sym::add).collect(), Sym::add(b)))
}

fn is_nonzero(s: &Sym) -> bool {
    if s.is_exact_zero() {
        return false;
    }
    if s.is_constant() {
        return super::approx::sign(s) != Some(Ordering::Equal)
            && approx(s, work_prec()).is_ok_and(|n| !n.is_zero());
    }
    !cancel(s).is_exact_zero()
}

/// Exact Gaussian elimination; `None` if the system is not linear.
fn linear_system(fs: &[Sym], vars: &[String]) -> R<Option<Vec<Assignment>>> {
    let mut rows: Vec<Vec<Sym>> = Vec::new();
    for f in fs {
        let Some((a, b)) = linear_coeffs(f, vars) else {
            return Ok(None);
        };
        let mut row = a;
        row.push(-b);
        rows.push(row);
    }
    let n = vars.len();
    let mut pivots = Vec::new();
    let mut r = 0;
    for col in 0..n {
        let Some(p) = (r..rows.len()).find(|&i| is_nonzero(&rows[i][col])) else {
            continue;
        };
        rows.swap(r, p);
        let pv = rows[r][col].clone();
        rows[r] = rows[r].iter().map(|v| cancel(&(v / &pv))).collect();
        for i in 0..rows.len() {
            if i != r && !rows[i][col].is_exact_zero() {
                let fct = rows[i][col].clone();
                let pr = rows[r].clone();
                rows[i] = rows[i]
                    .iter()
                    .zip(&pr)
                    .map(|(v, p)| cancel(&(v - &fct * p)))
                    .collect();
            }
        }
        pivots.push((r, col));
        r += 1;
        if r == rows.len() {
            break;
        }
    }
    record("Gaussian elimination", || {
        (
            Sym::raw_add(fs.to_vec()),
            Sym::raw_add(rows.iter().map(|row| Sym::raw_add(row.clone())).collect()),
        )
    });
    // Inconsistent rows: 0 = c ≠ 0.
    for row in rows.iter().skip(r) {
        if is_nonzero(&row[n]) {
            return Ok(Some(Vec::new()));
        }
    }
    let mut sol = Vec::new();
    for &(row, col) in &pivots {
        let mut v = rows[row][n].clone();
        for (j, var) in vars.iter().enumerate() {
            if j != col && !pivots.iter().any(|&(_, c)| c == j) {
                v = v - &rows[row][j] * Sym::var(var);
            }
        }
        sol.push((vars[col].clone(), tidy(&cancel(&v))));
    }
    sol.sort_by_key(|(v, _)| vars.iter().position(|w| w == v));
    Ok(Some(vec![sol]))
}

fn system_rec(fs: Vec<Sym>, vars: Vec<String>, ctx: &Ctx, depth: u32) -> R<Vec<Assignment>> {
    if depth > 8 {
        return fail("system too complicated");
    }
    let mut eqs = Vec::new();
    for f in fs {
        let (n, _) = numer_denom(&cancel(&f));
        let n = expand(&n);
        if n.is_exact_zero() {
            continue;
        }
        if vars.iter().all(|v| !n.has_var(v)) {
            if n.is_constant() && super::approx::sign(&n) == Some(Ordering::Equal) {
                continue;
            }
            return Ok(Vec::new());
        }
        eqs.push(n);
    }
    if eqs.is_empty() {
        return Ok(vec![Vec::new()]);
    }
    // 1. An equation linear in some variable with a nonzero constant
    // coefficient.
    for (i, e) in eqs.iter().enumerate() {
        for v in &vars {
            if let Some(c) = sym_coeffs(e, v)
                && c.len() == 2
                && c[1].is_constant()
                && is_nonzero(&c[1])
            {
                let val = -&c[0] / &c[1];
                return substitute_and_recurse(&eqs, i, v, &[val], &vars, ctx, depth);
            }
        }
    }
    // 2. An equation in a single variable.
    for (i, e) in eqs.iter().enumerate() {
        let present: Vec<&String> = vars.iter().filter(|v| e.has_var(v)).collect();
        if present.len() == 1 {
            let v = present[0].clone();
            let vals = solve_zero(e, &v, ctx, 0)?;
            let vals: Vec<Sym> = vals
                .into_iter()
                .filter(|s| !s.has_var(&ctx.param))
                .filter(|s| ctx.complex || is_real(s))
                .collect();
            return substitute_and_recurse(&eqs, i, &v, &vals, &vars, ctx, depth);
        }
    }
    // 3. Eliminate a variable with a resultant.
    if eqs.len() >= 2 {
        for v in &vars {
            let (Some(c1), Some(c2)) = (sym_coeffs(&eqs[0], v), sym_coeffs(&eqs[1], v)) else {
                continue;
            };
            if c1.len() < 2 || c2.len() < 2 {
                continue;
            }
            let res = expand(&resultant(&c1, &c2));
            if res.is_exact_zero() {
                continue;
            }
            record("resultant", || {
                (
                    Sym::raw_add(vec![eqs[0].clone(), eqs[1].clone()]),
                    res.clone(),
                )
            });
            let mut rest: Vec<Sym> = vec![res];
            rest.extend(eqs[2..].iter().cloned());
            let others: Vec<String> = vars.iter().filter(|w| *w != v).cloned().collect();
            let partial = system_rec(rest, others, ctx, depth + 1)?;
            let mut out = Vec::new();
            for s in partial {
                let mut e1 = eqs[0].clone();
                for (w, val) in &s {
                    e1 = e1.subs(w, val);
                }
                let e1 = expand(&e1);
                let vals = if e1.has_var(v) {
                    solve_zero(&e1, v, ctx, 0).unwrap_or_default()
                } else {
                    let mut e2 = eqs[1].clone();
                    for (w, val) in &s {
                        e2 = e2.subs(w, val);
                    }
                    solve_zero(&expand(&e2), v, ctx, 0).unwrap_or_default()
                };
                for val in vals {
                    let mut full = s.clone();
                    full.push((v.clone(), val));
                    out.push(full);
                }
            }
            return Ok(out);
        }
    }
    fail("cannot solve this system symbolically (try nsolve on each equation)")
}

fn substitute_and_recurse(
    eqs: &[Sym],
    skip: usize,
    v: &str,
    vals: &[Sym],
    vars: &[String],
    ctx: &Ctx,
    depth: u32,
) -> R<Vec<Assignment>> {
    let others: Vec<String> = vars.iter().filter(|w| *w != v).cloned().collect();
    let mut out = Vec::new();
    for val in vals {
        record("substitute", || (Sym::var(v), val.clone()));
        let rest: Vec<Sym> = eqs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != skip)
            .map(|(_, e)| e.subs(v, val))
            .collect();
        for s in system_rec(rest, others.clone(), ctx, depth + 1)? {
            let mut value = val.clone();
            for (w, wv) in &s {
                value = value.subs(w, wv);
            }
            let mut full = s.clone();
            full.push((v.to_string(), value));
            full.sort_by_key(|(name, _)| vars.iter().position(|w| w == name));
            out.push(full);
        }
    }
    Ok(out)
}

/// Resultant of two polynomials given by coefficients (lowest first).
pub fn resultant(a: &[Sym], b: &[Sym]) -> Sym {
    let m = a.len() - 1;
    let n = b.len() - 1;
    let size = m + n;
    let mut mat = vec![vec![Sym::zero(); size]; size];
    for i in 0..n {
        for (j, c) in a.iter().rev().enumerate() {
            mat[i][i + j] = c.clone();
        }
    }
    for i in 0..m {
        for (j, c) in b.iter().rev().enumerate() {
            mat[n + i][i + j] = c.clone();
        }
    }
    determinant(mat)
}

/// Determinant by Gaussian elimination over rational functions.
pub fn determinant(mut m: Vec<Vec<Sym>>) -> Sym {
    let n = m.len();
    let mut det = Sym::one();
    for col in 0..n {
        let Some(p) = (col..n).find(|&r| is_nonzero(&m[r][col])) else {
            return Sym::zero();
        };
        if p != col {
            m.swap(p, col);
            det = -det;
        }
        let pv = m[col][col].clone();
        det = det * &pv;
        for r in col + 1..n {
            if m[r][col].is_exact_zero() {
                continue;
            }
            let f = cancel(&(&m[r][col] / &pv));
            let prow = m[col].clone();
            for (c, pc) in prow.iter().enumerate().skip(col) {
                m[r][c] = cancel(&(&m[r][c] - &f * pc));
            }
        }
    }
    cancel(&det)
}
