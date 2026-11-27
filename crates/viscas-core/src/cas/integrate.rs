//! Symbolic integration (indefinite).
//!
//! Strategy, in order: linearity and constant factors; the table of
//! elementary antiderivatives (also for linear arguments `f(ax + b)`);
//! rational functions by partial fractions (logs and arctangents);
//! substitution `u = g(x)` when the integrand is `h(u)·u'`; integration
//! by parts for `P(x)·{e^x, sin, cos, sinh, cosh}` and
//! `P(x)·{ln, atan, asin, acos}`; power reduction of trig products.
//! The result has no constant of integration. Every result can be
//! checked by differentiation (the tests do).

use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

use super::algebra::{cancel, expand, numer_denom, partfrac};
use super::diff::diff;
use super::expr::{Const, Func, Node, Sym};
use super::poly::{Poly, sym_coeffs};
use super::simplify::simplify;
use super::steps::record;

const MAX_DEPTH: u32 = 8;

/// `∫ e dx`, or `None` when no antiderivative was found.
pub fn integrate(e: &Sym, x: &str) -> Option<Sym> {
    let r = int_rec(e, x, 0)?;
    let r = tidy(&r);
    record("antiderivative", || (e.clone(), r.clone()));
    Some(r)
}

fn tidy(r: &Sym) -> Sym {
    if r.size() > 400 {
        return r.clone();
    }
    let s = simplify(r);
    if s.size() <= r.size() { s } else { r.clone() }
}

/// `(a, b)` with `u = a·x + b`, a ≠ 0 free of x.
fn linear(u: &Sym, x: &str) -> Option<(Sym, Sym)> {
    let c = sym_coeffs(&expand(u), x)?;
    if c.len() != 2 || c[1].is_exact_zero() {
        return None;
    }
    Some((c[1].clone(), c[0].clone()))
}

fn int_rec(e: &Sym, x: &str, depth: u32) -> Option<Sym> {
    if depth > MAX_DEPTH {
        return None;
    }
    crate::number::check_interrupt().ok()?;
    let xv = Sym::var(x);
    if !e.has_var(x) {
        return Some(e * &xv);
    }
    match e.node() {
        Node::Add(ts) => {
            let mut out = Vec::with_capacity(ts.len());
            for t in ts {
                out.push(int_rec(t, x, depth + 1)?);
            }
            let r = Sym::add(out);
            record("linearity", || (e.clone(), r.clone()));
            return Some(r);
        }
        Node::Mul(fs) => {
            let (k, v): (Vec<Sym>, Vec<Sym>) = fs.iter().cloned().partition(|f| !f.has_var(x));
            if !k.is_empty() {
                let inner = Sym::mul(v);
                let r = Sym::mul(k) * int_rec(&inner, x, depth + 1)?;
                return Some(r);
            }
        }
        _ => {}
    }
    if let Some(r) = table(e, x) {
        record("table of integrals", || (e.clone(), r.clone()));
        return Some(r);
    }
    if let Some(r) = rational(e, x) {
        return Some(r);
    }
    if let Some(r) = substitution(e, x, depth) {
        return Some(r);
    }
    if let Some(r) = by_parts(e, x, depth) {
        return Some(r);
    }
    // Expanding products of sums can expose simpler terms.
    let ex = expand(e);
    if ex != *e && matches!(ex.node(), Node::Add(_)) {
        return int_rec(&ex, x, depth + 1);
    }
    if let Some(r) = trig_powers(e, x, depth) {
        return Some(r);
    }
    None
}

/// Elementary antiderivatives, with linear inner arguments.
fn table(e: &Sym, x: &str) -> Option<Sym> {
    if let Some(r) = reciprocal_squares(e, x) {
        return Some(r);
    }
    let xv = Sym::var(x);
    match e.node() {
        Node::Var(_) => Some(xv.powi(2) / Sym::int(2)),
        Node::Pow(b, n) if !n.has_var(x) => {
            let (a, _) = linear(b, x)?;
            if n.as_i64() == Some(-1) {
                return Some(Sym::func(Func::Abs, b.clone()).ln() / a);
            }
            let n1 = n + &Sym::one();
            Some(Sym::pow(b.clone(), n1.clone()) / (n1 * a))
        }
        Node::Pow(b, u) if !b.has_var(x) => {
            let (a, _) = linear(u, x)?;
            let lnb = if matches!(b.node(), Node::Const(Const::E)) {
                Sym::one()
            } else {
                b.ln()
            };
            Some(e / (a * lnb))
        }
        Node::Fn(f, u) => {
            let (a, _) = linear(u, x)?;
            let u = u.clone();
            let r = match f {
                Func::Sin => -u.apply(Func::Cos),
                Func::Cos => u.apply(Func::Sin),
                Func::Tan => -Sym::func(Func::Abs, u.apply(Func::Cos)).ln(),
                Func::Sinh => u.apply(Func::Cosh),
                Func::Cosh => u.apply(Func::Sinh),
                Func::Tanh => u.apply(Func::Cosh).ln(),
                Func::Ln => &u * u.ln() - &u,
                Func::Asin => &u * u.apply(Func::Asin) + (Sym::one() - u.powi(2)).sqrt(),
                Func::Acos => &u * u.apply(Func::Acos) - (Sym::one() - u.powi(2)).sqrt(),
                Func::Atan => {
                    &u * u.apply(Func::Atan) - (Sym::one() + u.powi(2)).ln() / Sym::int(2)
                }
                Func::Asinh => &u * u.apply(Func::Asinh) - (u.powi(2) + Sym::one()).sqrt(),
                Func::Acosh => &u * u.apply(Func::Acosh) - (u.powi(2) - Sym::one()).sqrt(),
                Func::Atanh => {
                    &u * u.apply(Func::Atanh) + (Sym::one() - u.powi(2)).ln() / Sym::int(2)
                }
                Func::Abs => &u * Sym::func(Func::Abs, u.clone()) / Sym::int(2),
                Func::Sign => Sym::func(Func::Abs, u.clone()),
                _ => return None,
            };
            Some(r / a)
        }
        _ => None,
    }
}

/// `1/cos²u`, `1/sin²u`, `1/cosh²u`.
fn reciprocal_squares(e: &Sym, x: &str) -> Option<Sym> {
    let (b, n) = e.base_exp();
    if n.as_i64() != Some(-2) {
        return None;
    }
    let Node::Fn(f, u) = b.node() else {
        return None;
    };
    let (a, _) = linear(u, x)?;
    match f {
        Func::Cos => Some(u.apply(Func::Tan) / a),
        Func::Sin => Some(-(u.apply(Func::Tan).recip()) / a),
        Func::Cosh => Some(u.apply(Func::Tanh) / a),
        _ => None,
    }
}

/// `1/√(quadratic)` forms and rational functions with rational
/// coefficients (partial fractions).
fn rational(e: &Sym, x: &str) -> Option<Sym> {
    if let Some(r) = inverse_sqrt_quadratic(e, x) {
        return Some(r);
    }
    let (n, d) = numer_denom(e);
    let pn = Poly::from_sym(&expand(&n), x)?;
    let pd = Poly::from_sym(&expand(&d), x)?;
    if pd.deg() == 0 {
        return None;
    }
    let pf = partfrac(e, x)?;
    record("partial fractions", || (e.clone(), pf.clone()));
    let _ = pn;
    let xv = Sym::var(x);
    let mut out = Vec::new();
    for t in pf.terms() {
        if let Some(p) = Poly::from_sym(&t, x) {
            // Polynomial part.
            let mut terms = Vec::new();
            for (i, c) in p.c.iter().enumerate() {
                let k = BigRational::from_integer((i as i64 + 1).into());
                terms.push(Sym::ratio(c / &k) * xv.powi(i as i64 + 1));
            }
            out.push(Sym::add(terms));
            continue;
        }
        out.push(partial_term(&t, x)?);
    }
    Some(Sym::add(out))
}

/// `∫ A/(x − r)^k` and `∫ (Bx + C)/(x² + px + q)`.
fn partial_term(t: &Sym, x: &str) -> Option<Sym> {
    let (n, d) = numer_denom(t);
    let pn = Poly::from_sym(&expand(&n), x)?;
    // d = f^k with f irreducible.
    let (base, k) = match d.node() {
        Node::Pow(b, k) => (b.clone(), k.as_i64()?),
        _ => (d.clone(), 1),
    };
    let (dc, base_poly) = {
        let pd = Poly::from_sym(&expand(&base), x)?;
        (BigRational::one(), pd)
    };
    let _ = dc;
    let xv = Sym::var(x);
    let fsym = base_poly.to_sym(&xv);
    match base_poly.deg() {
        1 => {
            let a = Sym::ratio(base_poly.c[1].clone());
            let num = Sym::ratio(pn.coeff(0));
            if k == 1 {
                Some(num * Sym::func(Func::Abs, fsym).ln() / a)
            } else {
                Some(num * Sym::pow(fsym, Sym::int(1 - k)) / (a * Sym::int(1 - k)))
            }
        }
        2 if k == 1 => {
            // (Bx + C)/(a x² + b x + c)
            let (a, b, c) = (
                base_poly.c[2].clone(),
                base_poly.c[1].clone(),
                base_poly.c[0].clone(),
            );
            let bb = pn.coeff(1);
            let cc = pn.coeff(0);
            // B/(2a) · ln|f| + (C − B b/(2a)) ∫ 1/f
            let two = BigRational::from_integer(2.into());
            let log_coef = &bb / (&two * &a);
            let rest = &cc - &bb * &b / (&two * &a);
            let disc = &b * &b - BigRational::from_integer(4.into()) * &a * &c;
            let mut out = Vec::new();
            if !log_coef.is_zero() {
                out.push(Sym::ratio(log_coef) * Sym::func(Func::Abs, fsym.clone()).ln());
            }
            if !rest.is_zero() {
                if disc.is_negative() {
                    // ∫ 1/f = 2/√(−Δ) · atan((2a x + b)/√(−Δ))
                    let s = Sym::ratio(-disc).sqrt();
                    let arg = (Sym::ratio(&two * &a) * &xv + Sym::ratio(b)) / &s;
                    out.push(Sym::ratio(rest) * Sym::int(2) / &s * arg.apply(Func::Atan));
                } else {
                    return None;
                }
            }
            Some(Sym::add(out))
        }
        _ => None,
    }
}

/// `∫ 1/√(a x² + b x + c)` → asin / asinh / acosh forms (b = 0 only).
fn inverse_sqrt_quadratic(e: &Sym, x: &str) -> Option<Sym> {
    let (b, n) = e.base_exp();
    if n.as_rational()? != BigRational::new((-1).into(), 2.into()) {
        return None;
    }
    let c = sym_coeffs(&expand(&b), x)?;
    if c.len() != 3 || !c[1].is_exact_zero() {
        return None;
    }
    let (a, k) = (c[2].as_rational()?, c[0].as_rational()?);
    let xv = Sym::var(x);
    let sa = Sym::ratio(a.abs()).sqrt();
    let sk = Sym::ratio(k.abs()).sqrt();
    let t = &sa * &xv / &sk;
    let r = match (a.is_positive(), k.is_positive()) {
        (false, true) => t.apply(Func::Asin) / sa,
        (true, true) => t.apply(Func::Asinh) / sa,
        (true, false) => t.apply(Func::Acosh) / sa,
        _ => return None,
    };
    record("inverse trigonometric form", || (e.clone(), r.clone()));
    Some(r)
}

/// Candidate inner functions for `u = g(x)`.
fn kernels(e: &Sym, x: &str, out: &mut Vec<Sym>) {
    if !e.has_var(x) {
        return;
    }
    match e.node() {
        Node::Fn(_, a) => {
            if a.as_var().is_none() && !out.contains(a) {
                out.push(a.clone());
            }
            if !out.contains(e) {
                out.push(e.clone());
            }
        }
        Node::Pow(b, n) => {
            if b.as_var().is_none() && b.has_var(x) && !out.contains(b) {
                out.push(b.clone());
            }
            if n.has_var(x) && !out.contains(n) {
                out.push(n.clone());
            }
            if !b.has_var(x) && !out.contains(e) {
                out.push(e.clone());
            }
        }
        _ => {}
    }
    for c in e.children() {
        kernels(&c, x, out);
    }
}

/// `∫ h(g(x))·g'(x) dx = H(g(x))`.
fn substitution(e: &Sym, x: &str, depth: u32) -> Option<Sym> {
    let mut ks = Vec::new();
    kernels(e, x, &mut ks);
    ks.sort_by_key(|k| std::cmp::Reverse(k.size()));
    let t = Sym::var("_u");
    for u in ks {
        if linear(&u, x).is_some() {
            continue;
        }
        let Ok(du) = diff(&u, x) else { continue };
        if du.is_exact_zero() {
            continue;
        }
        let q = cancel(&(e / &du));
        let h = q.replace(&u, &t);
        let h = if h.has_var(x) {
            // u = x^n: x^n appears as powers; try expressing x via u.
            let h2 = simplify(&q).replace(&u, &t);
            if h2.has_var(x) {
                continue;
            }
            h2
        } else {
            h
        };
        if let Some(hint) = int_rec(&h, "_u", depth + 1) {
            let r = hint.subs("_u", &u);
            record("substitution", || {
                (e.clone(), Sym::eq(Sym::var("u"), u.clone()))
            });
            return Some(r);
        }
    }
    None
}

/// Polynomial factor and the rest of a product.
fn split_poly(e: &Sym, x: &str) -> Option<(Sym, Sym)> {
    let mut poly = Vec::new();
    let mut other = Vec::new();
    for f in e.factors() {
        if super::poly::is_poly_in(&f, x) {
            poly.push(f);
        } else {
            other.push(f);
        }
    }
    if poly.is_empty() || other.len() != 1 {
        return None;
    }
    Some((Sym::mul(poly), other.pop()?))
}

/// Integration by parts.
fn by_parts(e: &Sym, x: &str, depth: u32) -> Option<Sym> {
    let (p, g) = split_poly(e, x)?;
    let exp_like = match g.node() {
        Node::Pow(b, _) => !b.has_var(x),
        Node::Fn(Func::Sin | Func::Cos | Func::Sinh | Func::Cosh, _) => true,
        _ => false,
    };
    let log_like = matches!(
        g.node(),
        Node::Fn(
            Func::Ln | Func::Atan | Func::Asin | Func::Acos | Func::Asinh | Func::Atanh,
            _
        )
    ) || matches!(g.node(), Node::Pow(b, _) if matches!(b.node(), Node::Fn(Func::Ln, _)));
    if exp_like {
        // ∫ p g = p G − ∫ p' G
        let gi = table(&g, x)?;
        let dp = diff(&p, x).ok()?;
        let rest = if dp.is_exact_zero() {
            Sym::zero()
        } else {
            int_rec(&expand(&(dp * &gi)), x, depth + 1)?
        };
        let r = &p * &gi - rest;
        record("integration by parts", || (e.clone(), r.clone()));
        return Some(r);
    }
    if log_like {
        // ∫ p g = P g − ∫ P g'
        let pi = int_rec(&p, x, depth + 1)?;
        let dg = diff(&g, x).ok()?;
        let inner = cancel(&(&pi * dg));
        let rest = int_rec(&inner, x, depth + 1)?;
        let r = &pi * &g - rest;
        record("integration by parts", || (e.clone(), r.clone()));
        return Some(r);
    }
    None
}

/// Power reduction: `sin² = (1 − cos 2u)/2`, `cos² = (1 + cos 2u)/2`,
/// odd powers `sin^(2k+1) = sin·(1 − cos²)^k`.
fn trig_powers(e: &Sym, x: &str, depth: u32) -> Option<Sym> {
    fn rewrite(e: &Sym) -> Sym {
        let m = if e.children().is_empty() {
            e.clone()
        } else {
            e.map(&mut rewrite)
        };
        if let Node::Pow(b, n) = m.node()
            && let Node::Fn(f @ (Func::Sin | Func::Cos), u) = b.node()
            && let Some(k) = n.as_i64()
            && k >= 2
        {
            let other = if *f == Func::Sin {
                Func::Cos
            } else {
                Func::Sin
            };
            if k % 2 == 1 {
                // f^(2j+1) = f·(1 − g²)^j
                let base = Sym::one() - u.apply(other).powi(2);
                return Sym::mul(vec![b.clone(), base.powi(k / 2)]);
            }
            let sign = if *f == Func::Sin { -1 } else { 1 };
            let half =
                (Sym::one() + Sym::int(sign) * (Sym::int(2) * u).apply(Func::Cos)) / Sym::int(2);
            return half.powi(k / 2);
        }
        m
    }
    let r = rewrite(e);
    if r == *e {
        // sin(a)·cos(b) products → sums.
        return product_to_sum(e, x, depth);
    }
    let ex = expand(&r);
    record("power reduction", || (e.clone(), ex.clone()));
    int_rec(&ex, x, depth + 1)
}

/// `sin a cos b = (sin(a+b) + sin(a−b))/2` and friends.
fn product_to_sum(e: &Sym, x: &str, depth: u32) -> Option<Sym> {
    let fs = e.factors();
    let trig: Vec<(Func, Sym)> = fs
        .iter()
        .filter_map(|f| match f.node() {
            Node::Fn(g @ (Func::Sin | Func::Cos), u) => Some((*g, u.clone())),
            _ => None,
        })
        .collect();
    if trig.len() != 2 || fs.len() != 2 {
        return None;
    }
    let ((f1, a), (f2, b)) = (trig[0].clone(), trig[1].clone());
    let half = Sym::half();
    let r = match (f1, f2) {
        (Func::Sin, Func::Cos) => half * ((&a + &b).apply(Func::Sin) + (&a - &b).apply(Func::Sin)),
        (Func::Cos, Func::Sin) => half * ((&a + &b).apply(Func::Sin) - (&a - &b).apply(Func::Sin)),
        (Func::Sin, Func::Sin) => half * ((&a - &b).apply(Func::Cos) - (&a + &b).apply(Func::Cos)),
        (Func::Cos, Func::Cos) => half * ((&a - &b).apply(Func::Cos) + (&a + &b).apply(Func::Cos)),
        _ => return None,
    };
    let ex = expand(&r);
    if ex == *e {
        return None;
    }
    record("product to sum", || (e.clone(), ex.clone()));
    int_rec(&ex, x, depth + 1)
}
