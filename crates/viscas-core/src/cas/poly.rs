//! Dense univariate polynomials over ℚ: arithmetic, gcd, square-free
//! decomposition, rational roots, factorization over ℤ (rational roots +
//! Kronecker's method for small degrees) and numeric roots (Aberth).

use num_bigint::BigInt;
use num_integer::Integer as _;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

use super::expr::{Node, Sym};

/// `c[i]` is the coefficient of `x^i`; no trailing zeros (zero = empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Poly {
    pub c: Vec<BigRational>,
}

fn q(n: i64) -> BigRational {
    BigRational::from_integer(BigInt::from(n))
}

/// Limits for Kronecker's factorization.
const KRONECKER_MAX_DEGREE: usize = 12;
const KRONECKER_MAX_TRIES: usize = 200_000;

impl Poly {
    pub fn new(mut c: Vec<BigRational>) -> Poly {
        while c.last().is_some_and(|x| x.is_zero()) {
            c.pop();
        }
        Poly { c }
    }

    pub fn from_ints(c: &[i64]) -> Poly {
        Poly::new(c.iter().map(|&x| q(x)).collect())
    }

    pub fn zero() -> Poly {
        Poly { c: Vec::new() }
    }

    pub fn constant(a: BigRational) -> Poly {
        Poly::new(vec![a])
    }

    pub fn x() -> Poly {
        Poly::from_ints(&[0, 1])
    }

    pub fn is_zero(&self) -> bool {
        self.c.is_empty()
    }

    /// Degree (0 for the zero polynomial).
    pub fn deg(&self) -> usize {
        self.c.len().saturating_sub(1)
    }

    pub fn lead(&self) -> BigRational {
        self.c.last().cloned().unwrap_or_else(BigRational::zero)
    }

    pub fn coeff(&self, i: usize) -> BigRational {
        self.c.get(i).cloned().unwrap_or_else(BigRational::zero)
    }

    pub fn add(&self, o: &Poly) -> Poly {
        let n = self.c.len().max(o.c.len());
        Poly::new((0..n).map(|i| self.coeff(i) + o.coeff(i)).collect())
    }

    pub fn sub(&self, o: &Poly) -> Poly {
        let n = self.c.len().max(o.c.len());
        Poly::new((0..n).map(|i| self.coeff(i) - o.coeff(i)).collect())
    }

    pub fn mul(&self, o: &Poly) -> Poly {
        if self.is_zero() || o.is_zero() {
            return Poly::zero();
        }
        let mut c = vec![BigRational::zero(); self.c.len() + o.c.len() - 1];
        for (i, a) in self.c.iter().enumerate() {
            if a.is_zero() {
                continue;
            }
            for (j, b) in o.c.iter().enumerate() {
                c[i + j] += a * b;
            }
        }
        Poly::new(c)
    }

    pub fn scale(&self, k: &BigRational) -> Poly {
        Poly::new(self.c.iter().map(|a| a * k).collect())
    }

    pub fn pow(&self, n: usize) -> Poly {
        let mut acc = Poly::constant(q(1));
        for _ in 0..n {
            acc = acc.mul(self);
        }
        acc
    }

    /// Euclidean division: `(quotient, remainder)`. `o` must be nonzero.
    pub fn divrem(&self, o: &Poly) -> (Poly, Poly) {
        assert!(!o.is_zero(), "polynomial division by zero");
        let mut r = self.c.clone();
        let dl = o.c.len();
        if r.len() < dl {
            return (Poly::zero(), self.clone());
        }
        let lead = o.lead();
        let mut quo = vec![BigRational::zero(); r.len() - dl + 1];
        for k in (0..quo.len()).rev() {
            let coef = &r[k + dl - 1] / &lead;
            if coef.is_zero() {
                continue;
            }
            for (j, b) in o.c.iter().enumerate() {
                r[k + j] -= &coef * b;
            }
            quo[k] = coef;
        }
        (Poly::new(quo), Poly::new(r))
    }

    pub fn monic(&self) -> Poly {
        if self.is_zero() {
            return self.clone();
        }
        let l = self.lead();
        Poly::new(self.c.iter().map(|a| a / &l).collect())
    }

    /// Monic gcd.
    pub fn gcd(&self, o: &Poly) -> Poly {
        let (mut a, mut b) = (self.clone(), o.clone());
        while !b.is_zero() {
            let (_, r) = a.divrem(&b);
            a = b;
            b = r;
        }
        a.monic()
    }

    pub fn derivative(&self) -> Poly {
        Poly::new(
            self.c
                .iter()
                .enumerate()
                .skip(1)
                .map(|(i, a)| a * q(i as i64))
                .collect(),
        )
    }

    pub fn eval(&self, x: &BigRational) -> BigRational {
        let mut acc = BigRational::zero();
        for a in self.c.iter().rev() {
            acc = acc * x + a;
        }
        acc
    }

    /// `(content, primitive integer polynomial with positive leading
    /// coefficient)` with `self = content · primitive`.
    pub fn primitive(&self) -> (BigRational, Poly) {
        if self.is_zero() {
            return (BigRational::zero(), Poly::zero());
        }
        let mut den = BigInt::one();
        for a in &self.c {
            den = den.lcm(a.denom());
        }
        let ints: Vec<BigInt> = self
            .c
            .iter()
            .map(|a| (a * BigRational::from_integer(den.clone())).to_integer())
            .collect();
        let mut g = BigInt::zero();
        for a in &ints {
            g = g.gcd(a);
        }
        if self.lead().is_negative() {
            g = -g;
        }
        let content = BigRational::new(g.clone(), den);
        let prim = Poly::new(
            ints.iter()
                .map(|a| BigRational::from_integer(a / &g))
                .collect(),
        );
        (content, prim)
    }

    /// Yun's square-free decomposition of a monic-normalized polynomial:
    /// `[(f1, 1), (f2, 2), …]` with `self ∝ ∏ fi^i`, each `fi` monic.
    pub fn square_free(&self) -> Vec<(Poly, usize)> {
        let mut out = Vec::new();
        if self.deg() == 0 {
            return out;
        }
        let f = self.monic();
        let df = f.derivative();
        let mut a = f.gcd(&df);
        let mut b = f.divrem(&a).0;
        let mut c = df.divrem(&a).0;
        let mut d = c.sub(&b.derivative());
        let mut i = 1;
        loop {
            let g = b.gcd(&d);
            if g.deg() > 0 {
                out.push((g.clone(), i));
            }
            b = b.divrem(&g).0;
            if b.deg() == 0 {
                break;
            }
            c = d.divrem(&g).0;
            d = c.sub(&b.derivative());
            i += 1;
            if i > 10_000 {
                break;
            }
        }
        a = Poly::zero();
        let _ = a;
        out
    }

    /// Rational roots (without multiplicity), ascending.
    pub fn rational_roots(&self) -> Vec<BigRational> {
        let mut out = Vec::new();
        if self.deg() == 0 {
            return out;
        }
        let (_, p) = self.primitive();
        // Zero roots.
        let mut k = 0;
        while p.c[k].is_zero() {
            k += 1;
        }
        if k > 0 {
            out.push(BigRational::zero());
        }
        let trimmed = Poly::new(p.c[k..].to_vec());
        if trimmed.deg() == 0 {
            return out;
        }
        let a0 = trimmed.c[0].to_integer();
        let an = trimmed.lead().to_integer();
        let (Some(dp), Some(dq)) = (divisors(&a0), divisors(&an)) else {
            return out;
        };
        if dp.len() * dq.len() > 200_000 {
            return out;
        }
        for pp in &dp {
            for qq in &dq {
                for s in [1, -1] {
                    let r = BigRational::new(pp * s, qq.clone());
                    if trimmed.eval(&r).is_zero() && !out.contains(&r) {
                        out.push(r);
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Factorization over ℚ: `(constant, [(irreducible primitive integer
    /// factor with positive lead, multiplicity)])`, factors sorted by
    /// degree. Factors of degree > `KRONECKER_MAX_DEGREE` (12) that could not
    /// be split are returned as they are.
    pub fn factor(&self) -> (BigRational, Vec<(Poly, usize)>) {
        let (content, _) = self.primitive();
        let mut out: Vec<(Poly, usize)> = Vec::new();
        if self.deg() == 0 {
            return (self.lead(), out);
        }
        for (sf, mult) in self.square_free() {
            let (_, prim) = sf.primitive();
            for f in factor_square_free(&prim) {
                out.push((f, mult));
            }
        }
        // The constant: content of self divided by the product of the
        // primitive factors' leading coefficients' contribution.
        let mut prod = Poly::constant(q(1));
        for (f, m) in &out {
            prod = prod.mul(&f.pow(*m));
        }
        let k = self.lead() / prod.lead();
        let _ = content;
        out.sort_by(|a, b| {
            a.0.deg()
                .cmp(&b.0.deg())
                .then_with(|| a.0.c.iter().rev().cmp(b.0.c.iter().rev()))
        });
        (k, out)
    }

    pub fn to_sym(&self, x: &Sym) -> Sym {
        let mut terms = Vec::new();
        for (i, a) in self.c.iter().enumerate() {
            if a.is_zero() {
                continue;
            }
            terms.push(Sym::ratio(a.clone()) * x.powi(i as i64));
        }
        Sym::add(terms)
    }

    /// The polynomial of an *expanded* expression in `var` with rational
    /// coefficients.
    pub fn from_sym(e: &Sym, var: &str) -> Option<Poly> {
        let mut c: Vec<BigRational> = Vec::new();
        for t in e.terms() {
            let (k, coef) = term_degree(&t, var)?;
            let coef = coef.as_rational()?;
            if c.len() <= k {
                c.resize(k + 1, BigRational::zero());
            }
            c[k] += coef;
        }
        Some(Poly::new(c))
    }

    /// All complex roots numerically (with multiplicity), by the Aberth
    /// method on each square-free factor.
    pub fn numeric_roots(&self) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        for (f, m) in self.square_free() {
            let roots = aberth(&f);
            for _ in 0..m {
                out.extend(roots.iter().copied());
            }
        }
        out
    }
}

/// `(k, coefficient)` of a term `coefficient · var^k`.
pub(crate) fn term_degree(t: &Sym, var: &str) -> Option<(usize, Sym)> {
    let mut k = 0usize;
    let mut rest = Vec::new();
    for f in t.factors() {
        let (b, e) = f.base_exp();
        if b.as_var() == Some(var) {
            let d = e.as_i64()?;
            if d < 0 {
                return None;
            }
            k += d as usize;
        } else if f.has_var(var) {
            return None;
        } else {
            rest.push(f);
        }
    }
    Some((k, Sym::mul(rest)))
}

/// Coefficients (lowest degree first) of an expanded expression as a
/// polynomial in `var` with coefficients free of `var`.
pub fn sym_coeffs(e: &Sym, var: &str) -> Option<Vec<Sym>> {
    let mut c: Vec<Vec<Sym>> = Vec::new();
    for t in e.terms() {
        let (k, coef) = term_degree(&t, var)?;
        if c.len() <= k {
            c.resize(k + 1, Vec::new());
        }
        c[k].push(coef);
    }
    let mut v: Vec<Sym> = c.into_iter().map(Sym::add).collect();
    while v.last().is_some_and(|s| s.is_exact_zero()) {
        v.pop();
    }
    if v.is_empty() {
        v.push(Sym::zero());
    }
    Some(v)
}

/// Positive divisors of `n ≠ 0` (`None` if `n` is too hard to factor or
/// has too many divisors).
pub fn divisors(n: &BigInt) -> Option<Vec<BigInt>> {
    let n = n.abs();
    if n.is_zero() {
        return None;
    }
    let fs = crate::number::factorize(&n).ok()?;
    let mut out = vec![BigInt::one()];
    for (p, k) in fs {
        let mut next = Vec::with_capacity(out.len() * (k as usize + 1));
        for d in &out {
            let mut pk = BigInt::one();
            for _ in 0..=k {
                next.push(d * &pk);
                pk *= &p;
            }
        }
        out = next;
        if out.len() > 100_000 {
            return None;
        }
    }
    out.sort();
    Some(out)
}

/// Irreducible factors of a primitive square-free integer polynomial.
fn factor_square_free(f: &Poly) -> Vec<Poly> {
    let mut out = Vec::new();
    let mut rest = f.clone();
    // Linear factors from rational roots.
    for r in f.rational_roots() {
        let lin = Poly::new(
            vec![-r.numer().clone(), r.denom().clone()]
                .into_iter()
                .map(BigRational::from_integer)
                .collect(),
        );
        let (quo, rem) = rest.divrem(&lin);
        if rem.is_zero() {
            rest = quo;
            out.push(lin);
        }
    }
    if rest.deg() >= 1 {
        let (_, prim) = rest.primitive();
        if prim.deg() >= 4 {
            out.extend(kronecker(&prim));
        } else {
            out.push(prim);
        }
    }
    out
}

/// Kronecker's method: splits off factors of degree ≤ n/2.
fn kronecker(f: &Poly) -> Vec<Poly> {
    let n = f.deg();
    if !(4..=KRONECKER_MAX_DEGREE).contains(&n) {
        return vec![f.clone()];
    }
    for d in 2..=n / 2 {
        if let Some(g) = kronecker_find(f, d) {
            let (h, _) = f.divrem(&g);
            let (_, h) = h.primitive();
            let (_, g) = g.primitive();
            let mut out = kronecker(&g);
            out.extend(kronecker(&h));
            return out;
        }
    }
    vec![f.clone()]
}

fn kronecker_find(f: &Poly, d: usize) -> Option<Poly> {
    // Points with few divisors of f(a).
    let mut cands: Vec<(usize, BigInt, Vec<BigInt>)> = Vec::new();
    for a in -12i64..=12 {
        let v = f.eval(&q(a)).to_integer();
        if v.is_zero() {
            continue;
        }
        let ds = divisors(&v)?;
        cands.push((ds.len(), BigInt::from(a), ds));
    }
    if cands.len() < d + 1 {
        return None;
    }
    cands.sort_by_key(|c| c.0);
    let pts: Vec<(BigInt, Vec<BigInt>)> = cands
        .into_iter()
        .take(d + 1)
        .map(|(_, a, ds)| (a, ds))
        .collect();
    let mut total: usize = 1;
    for (k, (_, ds)) in pts.iter().enumerate() {
        let m = if k == 0 { ds.len() } else { 2 * ds.len() };
        total = total.checked_mul(m)?;
    }
    if total > KRONECKER_MAX_TRIES {
        return None;
    }
    let mut idx = vec![0usize; d + 1];
    loop {
        crate::number::check_interrupt().ok()?;
        // Values: first point positive divisor (fixes the overall sign).
        let vals: Vec<BigRational> = pts
            .iter()
            .enumerate()
            .map(|(k, (_, ds))| {
                if k == 0 {
                    BigRational::from_integer(ds[idx[0]].clone())
                } else {
                    let i = idx[k];
                    let v = ds[i / 2].clone();
                    BigRational::from_integer(if i.is_multiple_of(2) { v } else { -v })
                }
            })
            .collect();
        let xs: Vec<BigRational> = pts
            .iter()
            .map(|(a, _)| BigRational::from_integer(a.clone()))
            .collect();
        let g = interpolate(&xs, &vals);
        if g.deg() == d && g.c.iter().all(|c| c.is_integer()) {
            let (_, r) = f.divrem(&g);
            if r.is_zero() {
                return Some(g);
            }
        }
        // Next index tuple.
        let mut k = 0;
        loop {
            if k > d {
                return None;
            }
            idx[k] += 1;
            let lim = if k == 0 {
                pts[0].1.len()
            } else {
                2 * pts[k].1.len()
            };
            if idx[k] < lim {
                break;
            }
            idx[k] = 0;
            k += 1;
        }
    }
}

/// Lagrange interpolation through `(xs[i], ys[i])`.
pub fn interpolate(xs: &[BigRational], ys: &[BigRational]) -> Poly {
    let mut acc = Poly::zero();
    for i in 0..xs.len() {
        let mut term = Poly::constant(ys[i].clone());
        for j in 0..xs.len() {
            if i == j {
                continue;
            }
            let denom = &xs[i] - &xs[j];
            let lin = Poly::new(vec![-&xs[j] / &denom, BigRational::one() / &denom]);
            term = term.mul(&lin);
        }
        acc = acc.add(&term);
    }
    acc
}

// ----- numeric roots ---------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct C(f64, f64);

impl C {
    fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    fn sub(self, o: C) -> C {
        C(self.0 - o.0, self.1 - o.1)
    }
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C(
            (self.0 * o.0 + self.1 * o.1) / d,
            (self.1 * o.0 - self.0 * o.1) / d,
        )
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
}

fn horner(c: &[f64], z: C) -> (C, C) {
    let mut p = C(0.0, 0.0);
    let mut dp = C(0.0, 0.0);
    for &a in c.iter().rev() {
        dp = dp.mul(z).add(p);
        p = p.mul(z).add(C(a, 0.0));
    }
    (p, dp)
}

/// Aberth–Ehrlich iteration on a square-free polynomial.
fn aberth(f: &Poly) -> Vec<(f64, f64)> {
    let n = f.deg();
    if n == 0 {
        return Vec::new();
    }
    let lead = f.lead().to_f64().unwrap_or(1.0);
    let c: Vec<f64> =
        f.c.iter()
            .map(|a| a.to_f64().unwrap_or(0.0) / lead)
            .collect();
    if n == 1 {
        return vec![(-c[0], 0.0)];
    }
    // Cauchy bound for the initial circle.
    let bound = 1.0 + c[..n].iter().fold(0.0f64, |m, a| m.max(a.abs()));
    let r = bound.min(1e6) * 0.5 + 0.1;
    let mut z: Vec<C> = (0..n)
        .map(|k| {
            let t = 2.0 * std::f64::consts::PI * k as f64 / n as f64 + 0.4;
            C(r * t.cos(), r * t.sin())
        })
        .collect();
    for _ in 0..500 {
        let mut maxstep: f64 = 0.0;
        for i in 0..n {
            let (p, dp) = horner(&c, z[i]);
            if p.abs() == 0.0 {
                continue;
            }
            let ratio = p.div(dp);
            let mut s = C(0.0, 0.0);
            for j in 0..n {
                if i != j {
                    let d = z[i].sub(z[j]);
                    if d.abs() > 0.0 {
                        s = s.add(C(1.0, 0.0).div(d));
                    }
                }
            }
            let w = ratio.div(C(1.0, 0.0).sub(ratio.mul(s)));
            z[i] = z[i].sub(w);
            maxstep = maxstep.max(w.abs() / z[i].abs().max(1.0));
        }
        if maxstep < 1e-15 {
            break;
        }
    }
    // Newton polish and clean tiny imaginary parts.
    z.iter()
        .map(|&zi| {
            let mut x = zi;
            for _ in 0..3 {
                let (p, dp) = horner(&c, x);
                if dp.abs() > 0.0 {
                    x = x.sub(p.div(dp));
                }
            }
            let im = if x.1.abs() <= 1e-12 * x.0.abs().max(1.0) {
                0.0
            } else {
                x.1
            };
            (x.0, im)
        })
        .collect()
}

/// Is the expression a polynomial (after expansion) in `var`?
pub fn is_poly_in(e: &Sym, var: &str) -> bool {
    match e.node() {
        Node::Var(_) | Node::Num(_) | Node::Const(_) => true,
        Node::Add(v) | Node::Mul(v) => v.iter().all(|t| is_poly_in(t, var)),
        Node::Pow(b, x) => {
            if !b.has_var(var) {
                return !x.has_var(var);
            }
            x.as_i64().is_some_and(|k| k >= 0) && is_poly_in(b, var)
        }
        _ => !e.has_var(var),
    }
}
