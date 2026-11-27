//! Truncated Laurent series in `t` (expansion variable) with symbolic
//! coefficients, and `series(f, x, a, n)`.
//!
//! A series is `Σ c_i t^(lo + i)` known exactly for exponents below
//! `hi`. Elementary functions compose through their Taylor series around
//! the constant term (`exp(c + s) = e^c·exp(s)`, `ln(c + s) = ln c +
//! ln(1 + s/c)`, …); inverse trig functions integrate the series of
//! their derivative.

use num_rational::BigRational;
use num_traits::{One, ToPrimitive};

use super::algebra::{cancel, expand};
use super::expr::{Const, Func, Node, Sym};
use super::steps::record;

/// A truncated Laurent series.
#[derive(Clone, Debug)]
pub struct Ser {
    pub lo: i64,
    /// Coefficients of `t^lo`, `t^(lo+1)`, …
    pub c: Vec<Sym>,
    /// Exponents `< hi` are exact.
    pub hi: i64,
}

fn is_zero(s: &Sym) -> bool {
    if s.is_exact_zero() {
        return true;
    }
    if s.is_num() {
        return s.is_zero();
    }
    cancel(s).is_exact_zero() || super::simplify::simplify(s).is_exact_zero()
}

fn norm(s: &Sym) -> Sym {
    if s.is_num() || s.size() < 4 {
        return s.clone();
    }
    let c = cancel(&expand(s));
    if c.size() <= s.size() { c } else { s.clone() }
}

impl Ser {
    fn new(lo: i64, c: Vec<Sym>, hi: i64) -> Ser {
        let mut s = Ser { lo, c, hi };
        s.c.truncate((hi - lo).max(0) as usize);
        s
    }

    pub fn constant(v: Sym, hi: i64) -> Ser {
        Ser::new(0, vec![v], hi)
    }

    fn coeff(&self, e: i64) -> Sym {
        if e < self.lo {
            return Sym::zero();
        }
        self.c
            .get((e - self.lo) as usize)
            .cloned()
            .unwrap_or_else(Sym::zero)
    }

    /// Drops leading zero coefficients.
    fn trim(mut self) -> Ser {
        while !self.c.is_empty() && is_zero(&self.c[0]) {
            self.c.remove(0);
            self.lo += 1;
        }
        if self.c.is_empty() {
            self.lo = self.hi;
        }
        self
    }

    /// Leading term `(coefficient, exponent)`, `None` if all known
    /// coefficients vanish.
    pub fn lead(&self) -> Option<(Sym, i64)> {
        let s = self.clone().trim();
        s.c.first().map(|c| (c.clone(), s.lo))
    }

    fn add(&self, o: &Ser) -> Ser {
        let lo = self.lo.min(o.lo);
        let hi = self.hi.min(o.hi);
        let c = (lo..hi)
            .map(|e| norm(&(self.coeff(e) + o.coeff(e))))
            .collect();
        Ser::new(lo, c, hi)
    }

    fn neg(&self) -> Ser {
        Ser::new(self.lo, self.c.iter().map(|c| -c).collect(), self.hi)
    }

    fn scale(&self, k: &Sym) -> Ser {
        Ser::new(
            self.lo,
            self.c.iter().map(|c| norm(&(c * k))).collect(),
            self.hi,
        )
    }

    fn mul(&self, o: &Ser) -> Ser {
        let a = self.clone().trim();
        let b = o.clone().trim();
        let lo = a.lo + b.lo;
        let hi = (a.hi + b.lo).min(b.hi + a.lo);
        let mut c = Vec::new();
        for e in lo..hi {
            let mut terms = Vec::new();
            for (i, ai) in a.c.iter().enumerate() {
                let j = e - lo - i as i64;
                if j < 0 {
                    break;
                }
                if let Some(bj) = b.c.get(j as usize) {
                    terms.push(ai * bj);
                }
            }
            c.push(norm(&Sym::add(terms)));
        }
        Ser::new(lo, c, hi)
    }

    /// `Σ a_k s^k` for a series `s` of positive order (composition).
    fn compose(coeffs: &[Sym], s: &Ser, hi: i64) -> Option<Ser> {
        let s = s.clone().trim();
        if s.c.is_empty() {
            return Some(Ser::constant(coeffs.first()?.clone(), hi));
        }
        if s.lo <= 0 {
            return None;
        }
        let mut acc = Ser::constant(coeffs[0].clone(), hi);
        let mut p = Ser::constant(Sym::one(), hi);
        for a in coeffs.iter().skip(1) {
            p = p.mul(&s);
            if p.lo >= hi {
                break;
            }
            if !a.is_exact_zero() {
                acc = acc.add(&p.scale(a));
            }
        }
        Some(acc)
    }

    fn inv(&self) -> Option<Ser> {
        let a = self.clone().trim();
        let c0 = a.c.first()?.clone();
        // a = c0 t^lo (1 + u)
        let rest = Ser::new(
            0,
            a.c.iter().map(|c| norm(&(c / &c0))).collect(),
            a.hi - a.lo,
        );
        let u = rest.add(&Ser::constant(Sym::minus_one(), rest.hi));
        let n = (rest.hi).max(1) as usize + 1;
        let coeffs: Vec<Sym> = (0..n)
            .map(|k| Sym::int(if k % 2 == 0 { 1 } else { -1 }))
            .collect();
        let s = Ser::compose(&coeffs, &u, rest.hi)?;
        let s = s.scale(&c0.recip());
        Some(Ser::new(s.lo - a.lo, s.c, s.hi - a.lo))
    }

    fn powi(&self, k: i64) -> Option<Ser> {
        if k < 0 {
            return self.inv()?.powi(-k);
        }
        let mut acc = Ser::constant(Sym::one(), self.hi.max(self.lo * k + (self.hi - self.lo)));
        for _ in 0..k {
            acc = acc.mul(self);
        }
        Some(acc)
    }

    /// `s^r` for rational r (the leading exponent times r must be an
    /// integer).
    fn powr(&self, r: &BigRational) -> Option<Ser> {
        if r.is_integer() {
            return self.powi(r.to_integer().to_i64()?);
        }
        let a = self.clone().trim();
        let c0 = a.c.first()?.clone();
        let lead_exp = BigRational::from_integer(a.lo.into()) * r;
        if !lead_exp.is_integer() {
            return None;
        }
        let rest = Ser::new(
            0,
            a.c.iter().map(|c| norm(&(c / &c0))).collect(),
            a.hi - a.lo,
        );
        let u = rest.add(&Ser::constant(Sym::minus_one(), rest.hi));
        let n = rest.hi.max(1) as usize + 1;
        let mut coeffs = Vec::with_capacity(n);
        let mut binom = BigRational::one();
        for k in 0..n {
            coeffs.push(Sym::ratio(binom.clone()));
            binom = binom * (r - BigRational::from_integer(k.into()))
                / BigRational::from_integer((k + 1).into());
        }
        let s = Ser::compose(&coeffs, &u, rest.hi)?;
        let s = s.scale(&Sym::pow(c0, Sym::ratio(r.clone())));
        let shift = lead_exp.to_integer().to_i64()?;
        Some(Ser::new(s.lo + shift, s.c, s.hi + shift))
    }

    fn deriv(&self) -> Ser {
        let c = self
            .c
            .iter()
            .enumerate()
            .map(|(i, c)| c * Sym::int(self.lo + i as i64))
            .collect();
        Ser::new(self.lo - 1, c, self.hi - 1)
    }

    /// Termwise integral (no `t^-1` term allowed) plus a constant.
    fn integral(&self, k: Sym) -> Option<Ser> {
        let mut out = Ser::constant(k, self.hi + 1);
        for (i, c) in self.c.iter().enumerate() {
            let e = self.lo + i as i64;
            if e == -1 {
                if is_zero(c) {
                    continue;
                }
                return None;
            }
            let term = Ser::new(e + 1, vec![norm(&(c / Sym::int(e + 1)))], self.hi + 1);
            out = out.add(&term);
        }
        Some(out)
    }

    /// Splits `c0 + s1` (constant term and positive-order rest); `None`
    /// for negative orders.
    fn split(&self) -> Option<(Sym, Ser)> {
        let a = self.clone().trim();
        if a.lo < 0 && !a.c.is_empty() {
            return None;
        }
        let c0 = a.coeff(0);
        let rest = a.add(&Ser::constant(-&c0, a.hi));
        Some((c0, rest))
    }

    pub fn to_sym(&self, t: &Sym) -> Sym {
        Sym::add(
            self.c
                .iter()
                .enumerate()
                .map(|(i, c)| c * t.powi(self.lo + i as i64))
                .collect(),
        )
    }
}

fn factorial_coeffs(n: usize, f: impl Fn(usize) -> Option<Sym>) -> Vec<Sym> {
    let mut fact = BigRational::one();
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        if k > 0 {
            fact *= BigRational::from_integer(k.into());
        }
        out.push(match f(k) {
            Some(s) => s / Sym::ratio(fact.clone()),
            None => Sym::zero(),
        });
    }
    out
}

/// Series of `e` in `t`, where the expansion variable `x` is replaced by
/// `x0(t)` (a series), exact below `t^hi`.
fn ser(e: &Sym, x: &str, x0: &Ser, hi: i64, depth: u32) -> Option<Ser> {
    if depth > 40 {
        return None;
    }
    crate::number::check_interrupt().ok()?;
    if !e.has_var(x) {
        return Some(Ser::constant(e.clone(), hi));
    }
    let n = (hi + 2).max(2) as usize;
    match e.node() {
        Node::Var(_) => Some(x0.clone()),
        Node::Add(ts) => {
            let mut acc = Ser::constant(Sym::zero(), hi);
            for t in ts {
                acc = acc.add(&ser(t, x, x0, hi, depth + 1)?);
            }
            Some(acc)
        }
        Node::Mul(fs) => {
            // Compute factors with some headroom for poles in other factors.
            let parts: Vec<Ser> = fs
                .iter()
                .map(|f| ser(f, x, x0, hi + 4, depth + 1))
                .collect::<Option<_>>()?;
            let lows: i64 = parts.iter().map(|p| p.clone().trim().lo.min(0)).sum();
            let mut acc = Ser::constant(Sym::one(), hi - lows + 4);
            for p in &parts {
                acc = acc.mul(p);
            }
            Some(Ser::new(acc.lo, acc.c.clone(), acc.hi.min(hi)))
        }
        Node::Pow(b, k) if !k.has_var(x) => {
            let bs = ser(b, x, x0, hi + 4, depth + 1)?;
            let r = k.as_rational()?;
            bs.powr(&r)
        }
        Node::Pow(b, k) if matches!(b.node(), Node::Const(Const::E)) => {
            let s = ser(k, x, x0, hi, depth + 1)?;
            exp_ser(&s, hi, n)
        }
        Node::Pow(b, k) => {
            // b^k = exp(k ln b)
            let l = Sym::mul(vec![k.clone(), b.ln()]);
            let s = ser(&l, x, x0, hi, depth + 1)?;
            exp_ser(&s, hi, n)
        }
        Node::Fn(f, u) => {
            let s = ser(u, x, x0, hi + 2, depth + 1)?;
            fn_ser(*f, &s, hi, n)
        }
        _ => None,
    }
}

fn exp_ser(s: &Ser, hi: i64, n: usize) -> Option<Ser> {
    let (c0, rest) = s.split()?;
    let coeffs = factorial_coeffs(n, |_| Some(Sym::one()));
    Some(Ser::compose(&coeffs, &rest, hi)?.scale(&c0.exp()))
}

fn fn_ser(f: Func, s: &Ser, hi: i64, n: usize) -> Option<Ser> {
    let (c0, rest) = s.split()?;
    match f {
        Func::Sin | Func::Cos => {
            let sin_c = factorial_coeffs(n, |k| {
                (k % 2 == 1).then(|| Sym::int(if k % 4 == 1 { 1 } else { -1 }))
            });
            let cos_c = factorial_coeffs(n, |k| {
                (k % 2 == 0).then(|| Sym::int(if k % 4 == 0 { 1 } else { -1 }))
            });
            let sr = Ser::compose(&sin_c, &rest, hi)?;
            let cr = Ser::compose(&cos_c, &rest, hi)?;
            let (sc, cc) = (c0.apply(Func::Sin), c0.apply(Func::Cos));
            Some(if f == Func::Sin {
                cr.scale(&sc).add(&sr.scale(&cc))
            } else {
                cr.scale(&cc).add(&sr.scale(&sc).neg())
            })
        }
        Func::Tan => {
            let si = fn_ser(Func::Sin, s, hi + 2, n + 2)?;
            let co = fn_ser(Func::Cos, s, hi + 2, n + 2)?;
            let r = si.mul(&co.inv()?);
            Some(Ser::new(r.lo, r.c.clone(), r.hi.min(hi)))
        }
        Func::Sinh | Func::Cosh | Func::Tanh => {
            let ep = exp_ser(s, hi + 2, n + 2)?;
            let em = exp_ser(&s.neg(), hi + 2, n + 2)?;
            let r = match f {
                Func::Sinh => ep.add(&em.neg()).scale(&Sym::half()),
                Func::Cosh => ep.add(&em).scale(&Sym::half()),
                _ => ep.add(&em.neg()).mul(&ep.add(&em).inv()?),
            };
            Some(Ser::new(r.lo, r.c.clone(), r.hi.min(hi)))
        }
        Func::Ln => {
            if is_zero(&c0) {
                return None;
            }
            let u = rest.scale(&c0.recip());
            let coeffs: Vec<Sym> = (0..n)
                .map(|k| {
                    if k == 0 {
                        Sym::zero()
                    } else {
                        Sym::rat(if k % 2 == 1 { 1 } else { -1 }, k as i64)
                    }
                })
                .collect();
            let r = Ser::compose(&coeffs, &u, hi)?;
            Some(r.add(&Ser::constant(c0.ln(), hi)))
        }
        Func::Atan | Func::Asin | Func::Acos | Func::Asinh | Func::Atanh => {
            // f(s) = f(c0) + ∫ f'(s) s' dt
            let ds = s.deriv();
            let one = Ser::constant(Sym::one(), hi + 1);
            let sq = s.mul(s);
            let dfs = match f {
                Func::Atan => one.add(&sq).inv()?,
                Func::Atanh => one.add(&sq.neg()).inv()?,
                Func::Asin | Func::Acos => one
                    .add(&sq.neg())
                    .powr(&BigRational::new((-1).into(), 2.into()))?,
                _ => one
                    .add(&sq)
                    .powr(&BigRational::new((-1).into(), 2.into()))?,
            };
            let integrand = dfs.mul(&ds);
            let r = integrand.integral(c0.apply(f))?;
            let r = if f == Func::Acos {
                // acos' = −asin'; constant acos(c0).
                integrand.neg().integral(c0.apply(Func::Acos))?
            } else {
                r
            };
            Some(Ser::new(r.lo, r.c.clone(), r.hi.min(hi)))
        }
        Func::Abs => {
            let lead = s.lead()?;
            match super::approx::sign(&lead.0)? {
                std::cmp::Ordering::Less => Some(s.neg()),
                _ => Some(s.clone()),
            }
        }
        _ => None,
    }
}

/// Series of `e` around `x = a` (`a` finite) in `t = x − a`, exact below
/// `t^hi`.
pub fn series_at(e: &Sym, x: &str, a: &Sym, hi: i64) -> Option<Ser> {
    let x0 = Ser::new(0, vec![a.clone(), Sym::one()], hi + 8);
    let mut extra = 0;
    loop {
        let s = ser(e, x, &x0, hi + extra, 0)?;
        let s = Ser::new(s.lo, s.c.clone(), s.hi.min(hi + extra));
        if s.lead().is_some() || extra >= 8 {
            return Some(s);
        }
        extra += 4;
    }
}

/// Series of `e` for `x → ±∞` in `t = 1/x` (`sign` = ±1).
pub fn series_at_infinity(e: &Sym, x: &str, sign: i64, hi: i64) -> Option<Ser> {
    let t = Sym::var("_t");
    let sub = e.subs(x, &(Sym::int(sign) / &t));
    series_at(&sub, "_t", &Sym::zero(), hi)
}

/// `series(e, x, a, n)`: the Taylor (or Laurent) polynomial of `e` at
/// `x = a` with the terms of order `< n` in `x − a`.
pub fn series(e: &Sym, x: &str, a: &Sym, n: i64) -> Option<Sym> {
    let s = series_at(e, x, a, n)?;
    let t = if a.is_exact_zero() {
        Sym::var(x)
    } else {
        Sym::var(x) - a
    };
    let c: Vec<Sym> = s.c.iter().map(super::simplify::simplify).collect();
    let r = Ser::new(s.lo, c, n.min(s.hi)).to_sym_raw(&t);
    record("series expansion", || (e.clone(), r.clone()));
    Some(r)
}

impl Ser {
    /// Sum with the powers of `t` kept unexpanded (`(x − 1)^2`).
    fn to_sym_raw(&self, t: &Sym) -> Sym {
        let terms: Vec<Sym> = self
            .c
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_exact_zero())
            .map(|(i, c)| c * t.powi(self.lo + i as i64))
            .collect();
        if matches!(t.node(), Node::Add(_)) {
            // Keep (x − a)^k as a factor: build without distributing.
            let pieces: Vec<Sym> = self
                .c
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.is_exact_zero())
                .map(|(i, c)| {
                    let k = self.lo + i as i64;
                    let p = match k {
                        0 => return c.clone(),
                        1 => t.clone(),
                        _ => Sym::raw_pow(t.clone(), Sym::int(k)),
                    };
                    if c.is_one() {
                        p
                    } else {
                        Sym::raw_mul(vec![c.clone(), p])
                    }
                })
                .collect();
            return Sym::raw_add(pieces);
        }
        Sym::add(terms)
    }
}
