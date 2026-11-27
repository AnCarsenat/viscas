//! Sparse multivariate polynomials over ℚ in *generalized variables*:
//! every non-numeric factor base (`x`, `sin(x)`, `√2`) is a variable.
//! Used for exact division (cancelling common factors), degrees and
//! coefficients.

use std::collections::BTreeMap;

use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

use super::expr::{Node, Sym};

/// A polynomial: monomial exponent vectors (indexed like `vars`) to
/// coefficients.
#[derive(Clone, Debug, PartialEq)]
pub struct MPoly {
    pub vars: Vec<Sym>,
    pub terms: BTreeMap<Vec<u32>, BigRational>,
}

/// Splits a factor into `(atom, exponent)` with a non-negative integer
/// exponent when possible.
fn atom_exp(f: &Sym) -> (Sym, u32) {
    if let Node::Pow(b, e) = f.node()
        && let Some(k) = e.as_i64()
        && k > 0
        && k < 1 << 16
    {
        return (b.clone(), k as u32);
    }
    (f.clone(), 1)
}

impl MPoly {
    /// Converts an *expanded* expression; `None` if a coefficient is not
    /// rational or a factor has a negative exponent.
    pub fn from_sym(e: &Sym) -> Option<MPoly> {
        let mut vars: Vec<Sym> = Vec::new();
        let mut raw: Vec<(Vec<(usize, u32)>, BigRational)> = Vec::new();
        for t in e.terms() {
            let mut coef = BigRational::one();
            let mut mono = Vec::new();
            for f in t.factors() {
                if let Some(n) = f.as_num() {
                    coef *= n.to_rational().filter(|_| n.is_exact() && n.is_real())?;
                    continue;
                }
                if let Node::Pow(_, ex) = f.node()
                    && ex.as_num().is_some_and(|n| n.is_real() && n.is_negative())
                {
                    return None;
                }
                let (a, k) = atom_exp(&f);
                let idx = match vars.iter().position(|v| *v == a) {
                    Some(i) => i,
                    None => {
                        vars.push(a);
                        vars.len() - 1
                    }
                };
                mono.push((idx, k));
            }
            raw.push((mono, coef));
        }
        // Sort variables canonically and remap.
        let mut order: Vec<usize> = (0..vars.len()).collect();
        order.sort_by(|&a, &b| vars[a].cmp(&vars[b]));
        let mut pos = vec![0; vars.len()];
        for (new, &old) in order.iter().enumerate() {
            pos[old] = new;
        }
        let sorted: Vec<Sym> = order.iter().map(|&i| vars[i].clone()).collect();
        let mut terms: BTreeMap<Vec<u32>, BigRational> = BTreeMap::new();
        for (mono, c) in raw {
            let mut ex = vec![0u32; sorted.len()];
            for (i, k) in mono {
                ex[pos[i]] += k;
            }
            *terms.entry(ex).or_insert_with(BigRational::zero) += c;
        }
        terms.retain(|_, c| !c.is_zero());
        Some(MPoly {
            vars: sorted,
            terms,
        })
    }

    pub fn is_zero(&self) -> bool {
        self.terms.is_empty()
    }

    /// Re-expresses over a (super)set of variables.
    pub fn with_vars(&self, vars: &[Sym]) -> Option<MPoly> {
        let map: Vec<usize> = self
            .vars
            .iter()
            .map(|v| vars.iter().position(|w| w == v))
            .collect::<Option<_>>()?;
        let terms = self
            .terms
            .iter()
            .map(|(ex, c)| {
                let mut e = vec![0u32; vars.len()];
                for (i, &k) in ex.iter().enumerate() {
                    e[map[i]] = k;
                }
                (e, c.clone())
            })
            .collect();
        Some(MPoly {
            vars: vars.to_vec(),
            terms,
        })
    }

    pub fn to_sym(&self) -> Sym {
        let terms = self
            .terms
            .iter()
            .map(|(ex, c)| {
                let mut fs = vec![Sym::ratio(c.clone())];
                for (i, &k) in ex.iter().enumerate() {
                    if k > 0 {
                        fs.push(self.vars[i].powi(k as i64));
                    }
                }
                Sym::mul(fs)
            })
            .collect();
        Sym::add(terms)
    }

    /// Leading term in lex order (last key of the map is the largest).
    fn lead(&self) -> Option<(&Vec<u32>, &BigRational)> {
        self.terms.iter().next_back()
    }

    /// Exact quotient `self / d`, or `None` if `d` does not divide `self`.
    pub fn exact_div(&self, d: &MPoly) -> Option<MPoly> {
        let mut vars = self.vars.clone();
        for v in &d.vars {
            if !vars.contains(v) {
                vars.push(v.clone());
            }
        }
        vars.sort();
        let mut r = self.with_vars(&vars)?;
        let d = d.with_vars(&vars)?;
        let (dl, dc) = {
            let (e, c) = d.lead()?;
            (e.clone(), c.clone())
        };
        let mut q: BTreeMap<Vec<u32>, BigRational> = BTreeMap::new();
        let mut guard = 0;
        while let Some((re, rc)) = r.lead().map(|(e, c)| (e.clone(), c.clone())) {
            guard += 1;
            if guard > 20_000 {
                return None;
            }
            let mut qe = Vec::with_capacity(re.len());
            for (a, b) in re.iter().zip(&dl) {
                if a < b {
                    return None;
                }
                qe.push(a - b);
            }
            let qc = rc / &dc;
            for (de, dcoef) in &d.terms {
                let ex: Vec<u32> = de.iter().zip(&qe).map(|(a, b)| a + b).collect();
                let entry = r.terms.entry(ex.clone()).or_insert_with(BigRational::zero);
                *entry -= &qc * dcoef;
                if entry.is_zero() {
                    r.terms.remove(&ex);
                }
            }
            q.insert(qe, qc);
        }
        Some(MPoly { vars, terms: q })
    }

    /// Degree in the variable `v` (0 if absent).
    pub fn degree_in(&self, v: &Sym) -> u32 {
        match self.vars.iter().position(|w| w == v) {
            Some(i) => self.terms.keys().map(|e| e[i]).max().unwrap_or(0),
            None => 0,
        }
    }

    /// Total degree.
    pub fn total_degree(&self) -> u32 {
        self.terms.keys().map(|e| e.iter().sum()).max().unwrap_or(0)
    }

    /// Is every term of the same total degree?
    pub fn is_homogeneous(&self) -> bool {
        let mut it = self.terms.keys().map(|e| e.iter().sum::<u32>());
        let Some(first) = it.next() else { return true };
        it.all(|d| d == first)
    }

    /// Gcd of the monomials and numeric content: `(coefficient, monomial)`.
    pub fn content(&self) -> (BigRational, Vec<u32>) {
        let mut mono: Option<Vec<u32>> = None;
        let mut num = num_bigint::BigInt::zero();
        let mut den = num_bigint::BigInt::one();
        for (e, c) in &self.terms {
            mono = Some(match mono {
                None => e.clone(),
                Some(m) => m.iter().zip(e).map(|(a, b)| *a.min(b)).collect(),
            });
            num = num_integer::Integer::gcd(&num, c.numer());
            den = num_integer::Integer::lcm(&den, c.denom());
        }
        let mut k = BigRational::new(num, den);
        if self.lead().is_some_and(|(_, c)| c.is_negative()) {
            k = -k;
        }
        (k, mono.unwrap_or_default())
    }

    /// Divides by a constant and a monomial (must divide every term).
    pub fn div_content(&self, k: &BigRational, mono: &[u32]) -> MPoly {
        let terms = self
            .terms
            .iter()
            .map(|(e, c)| (e.iter().zip(mono).map(|(a, b)| a - b).collect(), c / k))
            .collect();
        MPoly {
            vars: self.vars.clone(),
            terms,
        }
    }

    pub fn monomial_sym(&self, mono: &[u32]) -> Sym {
        Sym::mul(
            mono.iter()
                .enumerate()
                .filter(|(_, k)| **k > 0)
                .map(|(i, &k)| self.vars[i].powi(k as i64))
                .collect(),
        )
    }

    /// Number of variables that actually occur.
    pub fn used_vars(&self) -> Vec<usize> {
        (0..self.vars.len())
            .filter(|&i| self.terms.keys().any(|e| e[i] > 0))
            .collect()
    }

    /// Number of terms.
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }
}
