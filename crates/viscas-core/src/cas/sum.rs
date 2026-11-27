//! Closed forms of sums and products with symbolic (or infinite) upper
//! bounds: polynomials (Faulhaber, by interpolation), geometric terms,
//! telescoping rational terms; products of constants, linear factors
//! (`Γ`) and powers.

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};

use super::algebra::{expand, numer_denom, partfrac};
use super::expr::{Func, Sym};
use super::limit::{Dir, Lim, Point, as_infinity, limit};
use super::poly::{Poly, interpolate, sym_coeffs};
use super::simplify::simplify;
use super::steps::record;

/// `Σ_{k=a}^{b} f(k)`; `b` may be the infinity symbol. `None` when no
/// closed form is known.
pub fn summation(f: &Sym, k: &str, a: &Sym, b: &Sym) -> Option<Sym> {
    if let Some(s) = as_infinity(b) {
        if s < 0 {
            return None;
        }
        let n = Sym::var("_n");
        let partial = sum_closed(f, k, a, &n)?;
        let l = limit(&partial, "_n", &Point::PosInf, Dir::Both).ok()?;
        let r = l.to_sym();
        record("limit of the partial sums", || (partial.clone(), r.clone()));
        return Some(r);
    }
    let r = sum_closed(f, k, a, b)?;
    let r = tidy(&r);
    record("closed form of the sum", || (f.clone(), r.clone()));
    Some(r)
}

fn tidy(r: &Sym) -> Sym {
    let s = simplify(r);
    let f = super::algebra::factor(&s);
    if f.size() < s.size() { f } else { s }
}

fn sum_closed(f: &Sym, k: &str, a: &Sym, b: &Sym) -> Option<Sym> {
    if !f.has_var(k) {
        return Some(f * (b - a + Sym::one()));
    }
    let ex = expand(f);
    let mut out = Vec::new();
    for t in ex.terms() {
        out.push(sum_term(&t, k, a, b)?);
    }
    // A sum whose terms individually fail may telescope as a whole.
    Some(Sym::add(out))
}

fn sum_term(t: &Sym, k: &str, a: &Sym, b: &Sym) -> Option<Sym> {
    if !t.has_var(k) {
        return Some(t * (b - a + Sym::one()));
    }
    // c·k^p
    if let Some(c) = sym_coeffs(t, k)
        && c.len() <= 30
    {
        let p = c.len() - 1;
        let s = power_sum(p, a, b)?;
        record("sum of powers", || (t.clone(), c[p].clone() * s.clone()));
        return Some(&c[p] * s);
    }
    // c·r^(m k + q)
    if let Some(g) = geometric(t, k, a, b) {
        return Some(g);
    }
    telescoping(t, k, a, b)
}

/// `Σ_{k=a}^{b} k^p` via the degree-(p+1) polynomial through exact
/// partial sums.
fn power_sum(p: usize, a: &Sym, b: &Sym) -> Option<Sym> {
    // S(n) = Σ_{k=0}^{n} k^p is a polynomial in n.
    let pts: Vec<BigRational> = (0..=p as i64 + 1)
        .map(|i| BigRational::from_integer(i.into()))
        .collect();
    let mut acc = BigRational::zero();
    let mut vals = Vec::new();
    for n in 0..=p as i64 + 1 {
        acc += num_traits::pow(BigRational::from_integer(BigInt::from(n)), p);
        vals.push(acc.clone());
    }
    let s = interpolate(&pts, &vals);
    let n = Sym::var("_n");
    let poly = s.to_sym(&n);
    let upper = poly.subs("_n", b);
    let lower = poly.subs("_n", &(a - Sym::one()));
    Some(expand(&(upper - lower)))
}

fn geometric(t: &Sym, k: &str, a: &Sym, b: &Sym) -> Option<Sym> {
    let mut konst = Vec::new();
    let mut ratio: Option<Sym> = None;
    for f in t.factors() {
        if !f.has_var(k) {
            konst.push(f);
            continue;
        }
        let (base, e) = f.base_exp();
        if base.has_var(k) || ratio.is_some() {
            return None;
        }
        let c = sym_coeffs(&expand(&e), k)?;
        if c.len() != 2 {
            return None;
        }
        // base^(m k + q) = base^q · (base^m)^k
        konst.push(Sym::pow(base.clone(), c[0].clone()));
        ratio = Some(Sym::pow(base, c[1].clone()));
    }
    let r = ratio?;
    let c = Sym::mul(konst);
    if r.is_one() {
        return Some(c * (b - a + Sym::one()));
    }
    let s = c * (Sym::pow(r.clone(), b + &Sym::one()) - Sym::pow(r.clone(), a.clone()))
        / (&r - Sym::one());
    record("geometric series", || (t.clone(), s.clone()));
    Some(s)
}

/// Rational terms `Σ A_i/(k + α_i)` with `Σ A_i = 0` and integer shifts.
fn telescoping(t: &Sym, k: &str, a: &Sym, b: &Sym) -> Option<Sym> {
    let (_, d) = numer_denom(t);
    Poly::from_sym(&expand(&d), k)?;
    let pf = partfrac(t, k)?;
    let mut parts: Vec<(Sym, BigRational)> = Vec::new();
    for term in pf.terms() {
        let (n, d) = numer_denom(&term);
        if n.has_var(k) {
            return None;
        }
        let dp = Poly::from_sym(&expand(&d), k)?;
        if dp.deg() != 1 {
            return None;
        }
        // n/(c1 k + c0) = (n/c1)/(k + c0/c1)
        let c1 = dp.c[1].clone();
        let alpha = &dp.c[0] / &c1;
        parts.push((n / Sym::ratio(c1), alpha));
    }
    let total = simplify(&Sym::add(parts.iter().map(|p| p.0.clone()).collect()));
    if !total.is_exact_zero() {
        return None;
    }
    let alpha0 = parts.iter().map(|p| p.1.clone()).min()?;
    let kv = Sym::var(k);
    let mut out = Vec::new();
    for (coef, alpha) in &parts {
        let d = alpha - &alpha0;
        if !d.is_integer() {
            return None;
        }
        let d = d.to_integer().to_i64()?;
        if !(0..=50).contains(&d) {
            return None;
        }
        // Σ_{k=a}^{b} 1/(k + α0 + d) − Σ_{k=a}^{b} 1/(k + α0)
        //   = Σ_{j=1}^{d} 1/(b + α0 + j) − Σ_{j=0}^{d−1} 1/(a + α0 + j)
        let base = |s: &Sym, j: i64| (s + Sym::ratio(alpha0.clone()) + Sym::int(j)).recip();
        let mut terms = Vec::new();
        for j in 1..=d {
            terms.push(base(b, j));
        }
        for j in 0..d {
            terms.push(-base(a, j));
        }
        out.push(coef * Sym::add(terms));
    }
    let _ = kv;
    let r = Sym::add(out);
    record("telescoping sum", || (t.clone(), r.clone()));
    Some(r)
}

/// `∏_{k=a}^{b} f(k)`.
pub fn product(f: &Sym, k: &str, a: &Sym, b: &Sym) -> Option<Sym> {
    if as_infinity(b).is_some() {
        let n = Sym::var("_n");
        let partial = product(f, k, a, &n)?;
        return match limit(&partial, "_n", &Point::PosInf, Dir::Both).ok()? {
            Lim::Fin(s) => Some(s),
            l => Some(l.to_sym()),
        };
    }
    let count = b - a + Sym::one();
    let mut out = Vec::new();
    for fac in f.factors() {
        if !fac.has_var(k) {
            out.push(Sym::pow(fac, count.clone()));
            continue;
        }
        let (base, e) = fac.base_exp();
        if !base.has_var(k) {
            // ∏ base^(e(k)) = base^(Σ e(k))
            let s = summation(&e, k, a, b)?;
            out.push(Sym::pow(base, s));
            continue;
        }
        let ek = e.as_i64()?;
        // (c1 k + c0)^e: c1^count · Γ(b + α + 1)/Γ(a + α), α = c0/c1
        let p = Poly::from_sym(&expand(&base), k)?;
        if p.deg() != 1 {
            return None;
        }
        let c1 = Sym::ratio(p.c[1].clone());
        let alpha = Sym::ratio(&p.c[0] / &p.c[1]);
        let g =
            Sym::func(Func::Gamma, b + &alpha + Sym::one()) / Sym::func(Func::Gamma, a + &alpha);
        out.push(Sym::pow(Sym::pow(c1, count.clone()) * g, Sym::int(ek)));
    }
    let r = Sym::mul(out);
    record("closed form of the product", || (f.clone(), r.clone()));
    Some(r)
}
