//! Algebraic rewriting: expand, numerator/denominator, together, cancel,
//! factor, collect, coefficients, degree and partial fractions.

use num_rational::BigRational;
use num_traits::{One, ToPrimitive, Zero};

use super::expr::{Node, Sym};
use super::mpoly::MPoly;
use super::poly::{Poly, sym_coeffs};
use super::steps::record;

/// Maximum number of terms an expansion may produce.
const MAX_TERMS: usize = 20_000;

// ----- expand ----------------------------------------------------------------

/// Distributes products and positive integer powers over sums, everywhere
/// in the expression (`(x + 1)^2 → x^2 + 2x + 1`).
pub fn expand(e: &Sym) -> Sym {
    let r = expand_rec(e);
    record("expand", || (e.clone(), r.clone()));
    r
}

fn expand_rec(e: &Sym) -> Sym {
    match e.node() {
        Node::Add(ts) => Sym::add(ts.iter().map(expand_rec).collect()),
        Node::Mul(fs) => {
            let parts: Vec<Sym> = fs.iter().map(expand_rec).collect();
            distribute(&parts)
        }
        Node::Pow(b, x) => {
            let b = expand_rec(b);
            let x = expand_rec(x);
            if let Some(k) = x.as_i64()
                && matches!(b.node(), Node::Add(_))
            {
                if (2..=200).contains(&k) {
                    return expand_pow(&b, k as u32);
                }
                if (-200..=-2).contains(&k) {
                    return expand_pow(&b, (-k) as u32).recip();
                }
            }
            // e^(a + b) = e^a·e^b is not expanded (keeps solve/diff forms).
            Sym::pow(b, x)
        }
        Node::Fn(f, a) => Sym::func(*f, expand_rec(a)),
        Node::Rel(v, ops) => Sym::rel(v.iter().map(expand_rec).collect(), ops.clone()),
        _ => e.clone(),
    }
}

/// Product of already expanded factors, distributed.
fn distribute(parts: &[Sym]) -> Sym {
    let mut acc: Vec<Sym> = vec![Sym::one()];
    for p in parts {
        let ts = p.terms();
        if acc.len() * ts.len() > MAX_TERMS {
            return Sym::mul(parts.to_vec());
        }
        let mut next = Vec::with_capacity(acc.len() * ts.len());
        for a in &acc {
            for t in &ts {
                next.push(Sym::mul(vec![a.clone(), t.clone()]));
            }
        }
        // Combine like terms early to keep the lists short.
        acc = Sym::add(next).terms();
    }
    Sym::add(acc)
}

fn expand_pow(b: &Sym, k: u32) -> Sym {
    // Binary powering with distribution.
    let mut result = Sym::one();
    let mut base = b.clone();
    let mut n = k;
    while n > 0 {
        if n & 1 == 1 {
            result = distribute(&[result, base.clone()]);
        }
        n >>= 1;
        if n > 0 {
            base = distribute(&[base.clone(), base.clone()]);
        }
    }
    result
}

// ----- numerator / denominator -------------------------------------------------

/// `(numerator, denominator)` with `e = n/d` (no cancellation).
pub fn numer_denom(e: &Sym) -> (Sym, Sym) {
    match e.node() {
        Node::Num(n) => match n.to_rational() {
            Some(r) if n.is_exact() && n.is_real() => (
                Sym::big_int(r.numer().clone()),
                Sym::big_int(r.denom().clone()),
            ),
            _ => (e.clone(), Sym::one()),
        },
        Node::Mul(fs) => {
            let mut ns = Vec::new();
            let mut ds = Vec::new();
            for f in fs {
                let (n, d) = numer_denom(f);
                ns.push(n);
                ds.push(d);
            }
            (Sym::mul(ns), Sym::mul(ds))
        }
        Node::Pow(b, x) => {
            if let Some(n) = x.as_num()
                && n.is_real()
                && n.is_negative()
            {
                let pos = Sym::num(-n);
                if x.as_integer().is_some() {
                    let (bn, bd) = numer_denom(b);
                    return (Sym::pow(bd, pos.clone()), Sym::pow(bn, pos));
                }
                return (Sym::one(), Sym::pow(b.clone(), pos));
            }
            if x.is_negative_term() {
                return (Sym::one(), Sym::pow(b.clone(), -x));
            }
            if x.as_integer().is_some() {
                let (bn, bd) = numer_denom(b);
                return (Sym::pow(bn, x.clone()), Sym::pow(bd, x.clone()));
            }
            (e.clone(), Sym::one())
        }
        Node::Add(ts) => {
            let mut n = Sym::zero();
            let mut d = Sym::one();
            for t in ts {
                let (tn, td) = numer_denom(t);
                if td == d {
                    n = n + tn;
                } else if td.is_one() {
                    n = n + tn * &d;
                } else if d.is_one() {
                    n = n * &td + tn;
                    d = td;
                } else {
                    // Use the lcm of the factor lists to avoid blow-up.
                    let (l, md, mt) = lcm_factors(&d, &td);
                    n = n * md + tn * mt;
                    d = l;
                }
            }
            (n, d)
        }
        _ => (e.clone(), Sym::one()),
    }
}

/// Least common multiple of two products of powers: `(lcm, lcm/a, lcm/b)`.
fn lcm_factors(a: &Sym, b: &Sym) -> (Sym, Sym, Sym) {
    let fa: Vec<(Sym, Sym)> = a.factors().iter().map(Sym::base_exp).collect();
    let fb: Vec<(Sym, Sym)> = b.factors().iter().map(Sym::base_exp).collect();
    let mut l = Vec::new();
    let mut ma = Vec::new();
    let mut mb = Vec::new();
    let int_exp = |e: &Sym| e.as_i64().filter(|&k| k > 0);
    let mut used_b = vec![false; fb.len()];
    for (ba, ea) in &fa {
        if let Some(j) = fb.iter().position(|(bb, _)| bb == ba)
            && let (Some(ka), Some(kb)) = (int_exp(ea), int_exp(&fb[j].1))
        {
            used_b[j] = true;
            let k = ka.max(kb);
            l.push(ba.powi(k));
            if k > ka {
                ma.push(ba.powi(k - ka));
            }
            if k > kb {
                mb.push(ba.powi(k - kb));
            }
            continue;
        }
        l.push(Sym::pow(ba.clone(), ea.clone()));
        mb.push(Sym::pow(ba.clone(), ea.clone()));
    }
    for (j, (bb, eb)) in fb.iter().enumerate() {
        if !used_b[j] {
            l.push(Sym::pow(bb.clone(), eb.clone()));
            ma.push(Sym::pow(bb.clone(), eb.clone()));
        }
    }
    (Sym::mul(l), Sym::mul(ma), Sym::mul(mb))
}

pub fn numerator(e: &Sym) -> Sym {
    numer_denom(e).0
}

pub fn denominator(e: &Sym) -> Sym {
    numer_denom(e).1
}

/// Combines into a single fraction `n/d` (numerator expanded).
pub fn together(e: &Sym) -> Sym {
    let (n, d) = numer_denom(e);
    let r = expand(&n) / d;
    record("combine over a common denominator", || {
        (e.clone(), r.clone())
    });
    r
}

// ----- cancel ------------------------------------------------------------------

/// Univariate view of two multivariate polynomials sharing one atom.
fn univariate_pair(a: &MPoly, b: &MPoly) -> Option<(Sym, Poly, Poly)> {
    let mut vars: Vec<Sym> = a.vars.clone();
    for v in &b.vars {
        if !vars.contains(v) {
            vars.push(v.clone());
        }
    }
    if vars.len() != 1 {
        return None;
    }
    let to_poly = |m: &MPoly| -> Option<Poly> {
        let m = m.with_vars(&vars)?;
        let mut c: Vec<BigRational> = Vec::new();
        for (e, k) in &m.terms {
            let i = e[0] as usize;
            if c.len() <= i {
                c.resize(i + 1, BigRational::zero());
            }
            c[i] = k.clone();
        }
        Some(Poly::new(c))
    };
    Some((vars[0].clone(), to_poly(a)?, to_poly(b)?))
}

/// Cancels common factors of numerator and denominator:
/// `(x^2 - 1)/(x - 1) → x + 1`. Returns `n/d` with both expanded, the
/// denominator primitive with a positive leading coefficient.
pub fn cancel(e: &Sym) -> Sym {
    let r = cancel_rec(e);
    record("cancel common factors", || (e.clone(), r.clone()));
    r
}

fn cancel_rec(e: &Sym) -> Sym {
    if e.is_rel() {
        return e.map(&mut cancel_rec);
    }
    let (n, d) = numer_denom(e);
    let mut num = expand_rec(&n);
    let mut den = expand_rec(&d);
    if den.is_one() {
        return num;
    }
    if num.is_exact_zero() {
        return Sym::zero();
    }
    if let Some(k) = den.as_num() {
        return Sym::mul(vec![num, Sym::num(k.clone()).recip()]);
    }
    let (Some(mn), Some(md)) = (MPoly::from_sym(&num), MPoly::from_sym(&den)) else {
        return num / den;
    };
    if let Some((x, pn, pd)) = univariate_pair(&mn, &md) {
        let g = pn.gcd(&pd);
        let (qn, qd) = if g.deg() > 0 {
            (pn.divrem(&g).0, pd.divrem(&g).0)
        } else {
            (pn, pd)
        };
        let (k, qd) = qd.primitive();
        let qn = qn.scale(&(BigRational::one() / k));
        return finish(qn.to_sym(&x), qd.to_sym(&x));
    }
    // Multivariate: exact division, then each factor of the denominator.
    if let Some(q) = mn.exact_div(&md) {
        return q.to_sym();
    }
    let mut cur_n = mn;
    let mut cur_d = md;
    let fac = factor_rec(&d);
    for f in fac.factors() {
        let (b, k) = f.base_exp();
        let reps = k.as_i64().unwrap_or(1).max(1);
        let Some(fb) = MPoly::from_sym(&expand_rec(&b)) else {
            continue;
        };
        if fb.total_degree() == 0 {
            continue;
        }
        for _ in 0..reps {
            match (cur_n.exact_div(&fb), cur_d.exact_div(&fb)) {
                (Some(a), Some(c)) => {
                    cur_n = a;
                    cur_d = c;
                }
                _ => break,
            }
        }
    }
    num = cur_n.to_sym();
    den = cur_d.to_sym();
    let (k, _) = cur_d.content();
    if !k.is_zero() && !k.is_one() {
        let inv = Sym::ratio(BigRational::one() / &k);
        num = expand_rec(&(num * &inv));
        den = expand_rec(&(den * inv));
    }
    finish(num, den)
}

fn finish(num: Sym, den: Sym) -> Sym {
    if den.is_one() {
        num
    } else {
        Sym::mul(vec![num, den.recip()])
    }
}

// ----- factor ------------------------------------------------------------------

/// Factors polynomials over ℚ (integer polynomial factorization: content,
/// square-free decomposition, rational roots, Kronecker), homogeneous
/// bivariate polynomials, and rational functions (numerator and
/// denominator separately). Non-polynomial atoms (`sin x`) act as
/// variables.
pub fn factor(e: &Sym) -> Sym {
    let r = factor_rec(e);
    record("factor", || (e.clone(), r.clone()));
    r
}

fn factor_rec(e: &Sym) -> Sym {
    if e.is_constant() && e.is_num() {
        return e.clone();
    }
    if e.is_rel() {
        return e.map(&mut factor_rec);
    }
    let (n, d) = numer_denom(e);
    if !d.is_one() {
        let c = cancel_rec(e);
        let (n, d) = numer_denom(&c);
        let fnum = factor_poly(&n);
        let fden = factor_poly(&d);
        return divide_raw(fnum, fden);
    }
    factor_poly(&n)
}

fn divide_raw(n: Sym, d: Sym) -> Sym {
    if d.is_one() {
        return n;
    }
    let mut fs = n.factors();
    if n.is_one() {
        fs.clear();
    }
    // Move a numeric denominator into the coefficient.
    let mut dfs = d.factors();
    if let Some(k) = dfs.first().and_then(|f| f.as_num().cloned()) {
        dfs.remove(0);
        let c = Sym::num(k).recip();
        if let Some(first) = fs.first().and_then(|f| f.as_num().cloned()) {
            fs[0] = Sym::num(first) * c;
        } else {
            fs.insert(0, c);
        }
    }
    for f in dfs {
        let (b, x) = f.base_exp();
        fs.push(Sym::raw_pow(b, -x));
    }
    Sym::raw_mul(fs)
}

/// Rewrites `b^(k·u)` (constant b, integer k ≥ 2) as `(b^u)^k` so that
/// `e^(2x) − 1` factors in the atom `e^x`.
fn split_powers(e: &Sym) -> Sym {
    match e.node() {
        Node::Pow(b, h) if b.is_constant() && !h.is_constant() => {
            let (c, rest) = h.coeff_rest();
            if let Some(k) = c.to_rational().filter(|r| r.is_integer() && c.is_exact())
                && let Some(k) = k.to_integer().to_i64()
                && (2..=12).contains(&k)
            {
                return Sym::raw_pow(Sym::pow(b.clone(), rest), Sym::int(k));
            }
            e.clone()
        }
        Node::Add(ts) => Sym::raw_add(ts.iter().map(split_powers).collect()),
        Node::Mul(fs) => Sym::raw_mul(fs.iter().map(split_powers).collect()),
        _ => e.clone(),
    }
}

/// Factors a polynomial expression (no denominators).
fn factor_poly(e: &Sym) -> Sym {
    let ex = split_powers(&expand_rec(e));
    let Some(m) = MPoly::from_sym(&ex) else {
        return e.clone();
    };
    if m.is_zero() {
        return Sym::zero();
    }
    let (k, mono) = m.content();
    let p = m.div_content(&k, &mono);
    let mut out: Vec<Sym> = Vec::new();
    if !k.is_one() {
        out.push(Sym::ratio(k));
    }
    for (i, &deg) in mono.iter().enumerate() {
        if deg > 0 {
            out.push(m.vars[i].powi(deg as i64));
        }
    }
    let used = p.used_vars();
    let mut poly_factors: Vec<(Sym, usize)> = Vec::new();
    if used.len() == 1 {
        let x = p.vars[used[0]].clone();
        let mut c: Vec<BigRational> = Vec::new();
        for (ex, coef) in &p.terms {
            let i = ex[used[0]] as usize;
            if c.len() <= i {
                c.resize(i + 1, BigRational::zero());
            }
            c[i] = coef.clone();
        }
        let poly = Poly::new(c);
        let (kk, fs) = poly.factor();
        if !kk.is_one() {
            // Should be 1 after content removal; keep it if not.
            out.insert(0, Sym::ratio(kk));
        }
        for (f, mult) in fs {
            poly_factors.push((f.to_sym(&x), mult));
        }
    } else if used.len() == 2 && p.is_homogeneous() && p.total_degree() > 1 {
        poly_factors = factor_homogeneous(&p, used[0], used[1]);
    } else if !used.is_empty() {
        poly_factors.push((p.to_sym(), 1));
    }
    for (f, mult) in poly_factors {
        out.push(if mult == 1 {
            f
        } else {
            Sym::raw_pow(f, Sym::int(mult as i64))
        });
    }
    if out.is_empty() {
        return Sym::one();
    }
    // A leading -1 with a single sum: write -(…) as the sum negated
    // only when nothing else is there.
    Sym::raw_mul(out)
}

/// Factors a homogeneous polynomial in two variables by dehomogenizing.
fn factor_homogeneous(p: &MPoly, a: usize, b: usize) -> Vec<(Sym, usize)> {
    let n = p.total_degree() as usize;
    let mut c = vec![BigRational::zero(); n + 1];
    for (ex, coef) in &p.terms {
        c[ex[a] as usize] = coef.clone();
    }
    let poly = Poly::new(c);
    let (_, fs) = poly.factor();
    let (va, vb) = (p.vars[a].clone(), p.vars[b].clone());
    let mut out = Vec::new();
    let mut used = 0usize;
    for (f, mult) in fs {
        let m = f.deg();
        used += m * mult;
        // vb^m · f(va/vb)
        let terms =
            f.c.iter()
                .enumerate()
                .filter(|(_, k)| !k.is_zero())
                .map(|(i, k)| Sym::ratio(k.clone()) * va.powi(i as i64) * vb.powi((m - i) as i64))
                .collect();
        out.push((Sym::add(terms), mult));
    }
    if used < n {
        out.insert(0, (vb, n - used));
    }
    out
}

// ----- collect / coefficients / degree ---------------------------------------------

/// Coefficients of `e` as a polynomial in `x`, highest degree first.
pub fn coeffs(e: &Sym, x: &str) -> Option<Vec<Sym>> {
    let ex = expand(&cancel_rec(e));
    let mut c = sym_coeffs(&ex, x)?;
    c.reverse();
    Some(c)
}

/// Degree of `e` in `x` (`None` if not a polynomial in `x`).
pub fn degree(e: &Sym, x: &str) -> Option<usize> {
    let ex = expand_rec(e);
    let c = sym_coeffs(&ex, x)?;
    Some(c.len() - 1)
}

/// Collects powers of `x`: `a x + b x + c → (a + b)x + c`.
pub fn collect(e: &Sym, x: &str) -> Sym {
    let ex = expand_rec(e);
    let Some(c) = sym_coeffs(&ex, x) else {
        return ex;
    };
    let xv = Sym::var(x);
    let terms: Vec<Sym> = c
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, k)| !k.is_exact_zero())
        .map(|(i, k)| {
            let p = xv.powi(i as i64);
            if i == 0 {
                k.clone()
            } else if matches!(k.node(), Node::Add(_)) {
                Sym::raw_mul(vec![k.clone(), p])
            } else {
                k.clone() * p
            }
        })
        .collect();
    let r = Sym::raw_add(terms);
    record("collect powers", || (e.clone(), r.clone()));
    r
}

// ----- partial fractions -------------------------------------------------------------

/// Partial fraction decomposition of a rational function of `x` with
/// rational coefficients.
pub fn partfrac(e: &Sym, x: &str) -> Option<Sym> {
    let c = cancel_rec(e);
    let (n, d) = numer_denom(&c);
    let pn = Poly::from_sym(&expand_rec(&n), x)?;
    let pd = Poly::from_sym(&expand_rec(&d), x)?;
    if pd.is_zero() {
        return None;
    }
    let xv = Sym::var(x);
    if pd.deg() == 0 {
        return Some(c);
    }
    let (q, r) = pn.divrem(&pd);
    let (kd, facs) = pd.factor();
    let r = r.scale(&(BigRational::one() / &kd));
    // D = ∏ f^m (monic up to the constant kd).
    let mut dpoly = Poly::constant(BigRational::one());
    for (f, m) in &facs {
        dpoly = dpoly.mul(&f.pow(*m));
    }
    let n_unknowns = dpoly.deg();
    // Basis polynomials x^i · D / f^j.
    let mut basis: Vec<(usize, usize, usize, Poly)> = Vec::new();
    for (fi, (f, m)) in facs.iter().enumerate() {
        for j in 1..=*m {
            let cof = dpoly.divrem(&f.pow(j)).0;
            for i in 0..f.deg() {
                let mut xi = vec![BigRational::zero(); i + 1];
                xi[i] = BigRational::one();
                basis.push((fi, j, i, cof.mul(&Poly::new(xi))));
            }
        }
    }
    if basis.len() != n_unknowns {
        return None;
    }
    // Linear system: Σ a_k basis_k = r (coefficient-wise).
    let mut mat: Vec<Vec<BigRational>> = (0..n_unknowns)
        .map(|row| {
            let mut v: Vec<BigRational> = basis.iter().map(|b| b.3.coeff(row)).collect();
            v.push(r.coeff(row));
            v
        })
        .collect();
    let sol = solve_rational_system(&mut mat, n_unknowns)?;
    let mut terms = vec![q.to_sym(&xv)];
    for (fi, (f, m)) in facs.iter().enumerate() {
        for j in 1..=*m {
            let mut numer = Poly::zero();
            for (k, b) in basis.iter().enumerate() {
                if b.0 == fi && b.1 == j {
                    let mut xi = vec![BigRational::zero(); b.2 + 1];
                    xi[b.2] = sol[k].clone();
                    numer = numer.add(&Poly::new(xi));
                }
            }
            if numer.is_zero() {
                continue;
            }
            let fs = f.to_sym(&xv);
            terms.push(numer.to_sym(&xv) * Sym::pow(fs, Sym::int(-(j as i64))));
        }
    }
    let res = Sym::add(terms);
    record("partial fractions", || (e.clone(), res.clone()));
    Some(res)
}

/// Gauss–Jordan elimination on an augmented square rational system.
pub(crate) fn solve_rational_system(
    mat: &mut [Vec<BigRational>],
    n: usize,
) -> Option<Vec<BigRational>> {
    for col in 0..n {
        let piv = (col..n).find(|&r| !mat[r][col].is_zero())?;
        mat.swap(col, piv);
        let p = mat[col][col].clone();
        for v in mat[col].iter_mut() {
            *v = &*v / &p;
        }
        for r in 0..n {
            if r != col && !mat[r][col].is_zero() {
                let f = mat[r][col].clone();
                let pivot_row = mat[col].clone();
                for (v, pv) in mat[r].iter_mut().zip(pivot_row.iter()) {
                    *v -= &f * pv;
                }
            }
        }
    }
    Some(mat.iter().map(|row| row[n].clone()).collect())
}
