//! Calculus operators: `d/dx`, `f'`, `∫`, `∑`, `∏`.
//!
//! Sums and products with finite integer bounds are computed exactly.
//! Derivatives are symbolic in exact mode (`d/dx x^2 = 2x`, evaluated at
//! the current value of `x` when it has one) with a numeric fallback
//! (central finite differences) when the body has no symbolic derivative
//! and in `float64` (plot) mode. Indefinite integrals are symbolic
//! ([`crate::cas::integrate`]). Definite integrals are computed
//! numerically (adaptive Gauss–Kronrod 7/15, infinite bounds by
//! substitution) and, in exact mode, also from an antiderivative: the
//! exact value is returned when it agrees with the numeric one. `∑`/`∏`
//! with symbolic or infinite bounds use closed forms
//! ([`crate::cas::summation`], [`crate::cas::product`]).

use num_traits::ToPrimitive;

use super::env::canonical_name;
use super::error::{ErrorKind, EvalError, EvalResult, NumSpan};
use super::interp::Evaluator;
use super::ops::Arith;
use super::value::Value;
use crate::number::Number;
use crate::syntax::{BigOpKind, Expr, ExprKind, Span, UnaryOp};

/// Highest derivative order computed numerically.
const MAX_NUMERIC_ORDER: u32 = 6;

impl Evaluator<'_> {
    /// Evaluates `body` with `var = t` (as `f64`) and returns a real `f64`.
    fn eval_at(&mut self, var: &str, body: &Expr, t: f64, span: Span) -> EvalResult<f64> {
        let saved = self.opts.float64;
        self.opts.float64 = true;
        self.push_scope(vec![(var.to_string(), Value::num(Number::F64(t)))]);
        let r = self.eval(body);
        self.pop_scope();
        self.opts.float64 = saved;
        self.real_f64(&r?, span)
    }

    /// `d/dx expr` (order `n`) at the current value of `x`.
    pub(crate) fn eval_derivative(
        &mut self,
        var: &str,
        order: u32,
        body: &Expr,
        span: Span,
    ) -> EvalResult<Value> {
        let var = canonical_name(var).to_string();
        if !self.opts.float64
            && let Some(v) = self.symbolic_derivative(&var, order, body, span)?
        {
            return Ok(v);
        }
        let Some(at) = self.lookup_known(&var, span)? else {
            return Err(EvalError::unsupported(
                format!(
                    "`d/d{var}` needs a value for `{var}` (symbolic derivatives need the CAS, not implemented yet)"
                ),
                span,
            ));
        };
        self.map_value(at, span, &mut |ev, x| {
            let x0 = ev.real_f64(&x, span)?;
            let d = ev.numeric_derivative(order, x0, span, &mut |ev, t| {
                ev.eval_at(&var, body, t, span)
            })?;
            Ok(Value::Number(Number::f64(d).at(span)?))
        })
    }

    /// `d^n/dx^n body` symbolically: `None` when the body has no symbolic
    /// derivative (the caller falls back to numeric differentiation).
    fn symbolic_derivative(
        &mut self,
        var: &str,
        order: u32,
        body: &Expr,
        span: Span,
    ) -> EvalResult<Option<Value>> {
        use crate::cas::{Sym, diff_n};
        let x = Value::Expr(super::value::SymValue::new(Sym::var(var)));
        let saved = self.symbolic;
        self.symbolic = true;
        self.push_scope(vec![(var.to_string(), x)]);
        let r = self.eval(body);
        self.pop_scope();
        self.symbolic = saved;
        let Ok(v) = r else { return Ok(None) };
        let Some(s) = v.as_sym() else { return Ok(None) };
        let Ok(d) = diff_n(&s, var, order) else {
            return Ok(None);
        };
        crate::cas::steps::record("derivative", || (s.clone(), d.clone()));
        match self.lookup_known(var, span)? {
            None => self.sym_result(d, false, span).map(Some),
            Some(at) => {
                let r = self.map_value(at, span, &mut |ev, a| match a.as_sym() {
                    Some(a) => ev.sym_result(d.subs(var, &a), true, span),
                    None => Err(EvalError::ty(
                        format!(
                            "`d/d{var}` needs a number for `{var}`, got a {}",
                            a.type_name()
                        ),
                        span,
                    )),
                });
                r.map(Some)
            }
        }
    }

    /// `f'(x)` for a one-parameter user function, symbolically: `None`
    /// to fall back to numeric differentiation.
    pub(crate) fn symbolic_prime(
        &mut self,
        f: &std::sync::Arc<super::value::Function>,
        order: u32,
        arg: &Value,
        span: Span,
    ) -> EvalResult<Option<Value>> {
        use crate::cas::{Sym, diff_n};
        let super::value::Function::User(u) = &**f else {
            return Ok(None);
        };
        if u.params.len() != 1 || self.opts.float64 {
            return Ok(None);
        }
        let p = u.params[0].clone();
        let x = Value::Expr(super::value::SymValue::new(Sym::var(&p)));
        let saved = self.symbolic;
        self.symbolic = true;
        let r = self.call_function(f, vec![x], span);
        self.symbolic = saved;
        let Ok(v) = r else { return Ok(None) };
        let Some(s) = v.as_sym() else { return Ok(None) };
        let Ok(d) = diff_n(&s, &p, order) else {
            return Ok(None);
        };
        let r = self.map_value(arg.clone(), span, &mut |ev, a| match a.as_sym() {
            Some(a) => ev.sym_result(d.subs(&p, &a), true, span),
            None => Err(EvalError::ty(
                format!("`f'` needs a number, got a {}", a.type_name()),
                span,
            )),
        });
        r.map(Some)
    }

    /// Numeric derivative of order `n` of `f` at `x`: 5-point stencils for
    /// orders 1 and 2, binomial central differences above.
    pub(crate) fn numeric_derivative(
        &mut self,
        n: u32,
        x: f64,
        span: Span,
        f: &mut dyn FnMut(&mut Self, f64) -> EvalResult<f64>,
    ) -> EvalResult<f64> {
        if n == 0 {
            return f(self, x);
        }
        if n > MAX_NUMERIC_ORDER {
            return Err(EvalError::unsupported(
                format!("numeric derivatives are limited to order {MAX_NUMERIC_ORDER}"),
                span,
            ));
        }
        let scale = x.abs().max(1.0);
        let d = match n {
            1 => {
                let h = 1e-3 * scale;
                let (a, b) = (f(self, x - 2.0 * h)?, f(self, x - h)?);
                let (c, d) = (f(self, x + h)?, f(self, x + 2.0 * h)?);
                (a - 8.0 * b + 8.0 * c - d) / (12.0 * h)
            }
            2 => {
                let h = 2e-3 * scale;
                let (a, b, c) = (f(self, x - 2.0 * h)?, f(self, x - h)?, f(self, x)?);
                let (d, e) = (f(self, x + h)?, f(self, x + 2.0 * h)?);
                (-a + 16.0 * b - 30.0 * c + 16.0 * d - e) / (12.0 * h * h)
            }
            _ => {
                let h = f64::EPSILON.powf(1.0 / (n as f64 + 2.0)) * scale;
                let mut sum = 0.0;
                let mut binom = 1.0;
                for k in 0..=n {
                    let t = x + (n as f64 / 2.0 - k as f64) * h;
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    sum += sign * binom * f(self, t)?;
                    binom = binom * (n - k) as f64 / (k + 1) as f64;
                }
                sum / h.powi(n as i32)
            }
        };
        if d.is_finite() {
            Ok(d)
        } else {
            Err(EvalError::number(
                crate::number::NumberError::Domain("derivative does not exist here"),
                span,
            ))
        }
    }

    /// `∫(a, b) body dx` (numeric) — indefinite integrals need the CAS.
    pub(crate) fn eval_integral(
        &mut self,
        bounds: Option<&(Expr, Expr)>,
        body: &Expr,
        var: &str,
        span: Span,
    ) -> EvalResult<Value> {
        let var = canonical_name(var).to_string();
        let Some((lo, hi)) = bounds else {
            // Indefinite: symbolic antiderivative (no constant).
            let found = if self.opts.float64 {
                None
            } else {
                self.symbolic_body(&var, body)
                    .and_then(|f| crate::cas::integrate(&f, &var))
            };
            return match found {
                Some(r) => self.sym_result(r, false, span),
                None => Err(EvalError::unsupported(
                    "no antiderivative found; give bounds for a numeric value: ∫(a, b) f(x) dx",
                    span,
                )),
            };
        };
        let a = self.bound(lo)?;
        let b = self.bound(hi)?;
        if a.is_nan() || b.is_nan() {
            return Ok(Value::Undefined);
        }
        let v = if a == b {
            0.0
        } else if a > b {
            -self.integrate(&var, body, b, a, span)?
        } else {
            self.integrate(&var, body, a, b, span)?
        };
        if !self.opts.float64
            && let Some(exact) = self.exact_definite(&var, body, lo, hi, v)
        {
            return self.sym_result(exact, false, span);
        }
        Ok(Value::Number(Number::f64(v).at(span)?))
    }

    /// `body` as a symbolic expression in `var` (symbolic mode).
    pub(crate) fn symbolic_body(&mut self, var: &str, body: &Expr) -> Option<crate::cas::Sym> {
        use crate::cas::Sym;
        let x = Value::Expr(super::value::SymValue::new(Sym::var(var)));
        let saved = self.symbolic;
        self.symbolic = true;
        self.push_scope(vec![(var.to_string(), x)]);
        let r = self.eval(body);
        self.pop_scope();
        self.symbolic = saved;
        r.ok()?.as_sym()
    }

    /// A bound as a symbolic value (`∞` as the infinity symbol).
    fn sym_bound(&mut self, e: &Expr) -> Option<crate::cas::Sym> {
        let saved = self.symbolic;
        self.symbolic = true;
        let r = self.eval(e);
        self.symbolic = saved;
        r.ok()?.as_sym()
    }

    /// Exact value of a definite integral by an antiderivative, accepted
    /// only if it agrees with the numeric value (guards against
    /// antiderivatives that jump inside the interval).
    fn exact_definite(
        &mut self,
        var: &str,
        body: &Expr,
        lo: &Expr,
        hi: &Expr,
        numeric: f64,
    ) -> Option<crate::cas::Sym> {
        use crate::cas::limit::{Dir, Lim, Point, as_infinity, limit};
        let f = self.symbolic_body(var, body)?;
        let anti = crate::cas::integrate(&f, var)?;
        let at = |ev: &mut Self, e: &Expr| -> Option<crate::cas::Sym> {
            let b = ev.sym_bound(e)?;
            match as_infinity(&b) {
                Some(s) => {
                    let p = if s > 0 { Point::PosInf } else { Point::NegInf };
                    match limit(&anti, var, &p, Dir::Both).ok()? {
                        Lim::Fin(v) => Some(v),
                        _ => None,
                    }
                }
                None => {
                    let v = anti.subs(var, &b);
                    (!v.has_undefined()).then_some(v)
                }
            }
        };
        let (a, b) = (at(self, lo)?, at(self, hi)?);
        let r = crate::cas::simplify(&(b - a));
        let approx = crate::cas::approx_f64(&r)?;
        ((approx - numeric).abs() <= 1e-7 * numeric.abs().max(1.0)).then_some(r)
    }

    /// An integral bound: `∞`, `-∞` or a real number.
    fn bound(&mut self, e: &Expr) -> EvalResult<f64> {
        let is_inf = |e: &Expr| matches!(&*e.kind, ExprKind::Ident(n) if canonical_name(n) == "∞");
        if is_inf(e) {
            return Ok(f64::INFINITY);
        }
        if let ExprKind::Unary { op, expr } = &*e.kind
            && is_inf(expr)
        {
            return Ok(match op {
                UnaryOp::Neg => f64::NEG_INFINITY,
                _ => f64::INFINITY,
            });
        }
        let v = self.eval(e)?;
        self.real_f64(&v, e.span)
    }

    /// Adaptive Gauss–Kronrod on `[a, b]` (`a < b`, possibly infinite).
    fn integrate(&mut self, var: &str, body: &Expr, a: f64, b: f64, span: Span) -> EvalResult<f64> {
        // Map to a finite interval [ta, tb] with x = map(t), dx = jac(t) dt.
        type Map = Box<dyn Fn(f64) -> (f64, f64)>;
        let (ta, tb, map): (f64, f64, Map) = match (a.is_finite(), b.is_finite()) {
            (true, true) => (a, b, Box::new(|t| (t, 1.0))),
            (true, false) => (
                0.0,
                1.0,
                Box::new(move |t: f64| (a + t / (1.0 - t), 1.0 / ((1.0 - t) * (1.0 - t)))),
            ),
            (false, true) => (
                0.0,
                1.0,
                Box::new(move |t: f64| (b - t / (1.0 - t), 1.0 / ((1.0 - t) * (1.0 - t)))),
            ),
            (false, false) => (
                -1.0,
                1.0,
                Box::new(|t: f64| {
                    let d = 1.0 - t * t;
                    (t / d, (1.0 + t * t) / (d * d))
                }),
            ),
        };
        let mut g = |ev: &mut Self, t: f64| -> EvalResult<f64> {
            let (x, jac) = map(t);
            let y = ev.eval_at(var, body, x, span)?;
            if !y.is_finite() {
                return Err(EvalError::number(
                    crate::number::NumberError::Domain("integrand is undefined in the interval"),
                    span,
                ));
            }
            Ok(y * jac)
        };
        let first = gk15(self, &mut g, ta, tb)?;
        let mut parts = vec![(ta, tb, first.0, first.1)];
        for _ in 0..400 {
            let total: f64 = parts.iter().map(|p| p.2).sum();
            let err: f64 = parts.iter().map(|p| p.3).sum();
            if err <= 1e-11_f64.max(1e-11 * total.abs()) {
                return Ok(total);
            }
            let (k, _) = parts
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.3.total_cmp(&y.1.3))
                .expect("non-empty");
            let (lo, hi, _, _) = parts.swap_remove(k);
            let mid = 0.5 * (lo + hi);
            let l = gk15(self, &mut g, lo, mid)?;
            let r = gk15(self, &mut g, mid, hi)?;
            parts.push((lo, mid, l.0, l.1));
            parts.push((mid, hi, r.0, r.1));
        }
        let total: f64 = parts.iter().map(|p| p.2).sum();
        let err: f64 = parts.iter().map(|p| p.3).sum();
        if err <= 1e-6 * total.abs().max(1.0) {
            Ok(total)
        } else {
            Err(EvalError::other(
                "integral did not converge (divergent or highly oscillating?)",
                span,
            ))
        }
    }

    /// `∑(k = from, to) body` / `∏`, exact for exact terms.
    pub(crate) fn eval_bigop(
        &mut self,
        op: BigOpKind,
        var: &str,
        from: &Expr,
        to: &Expr,
        body: &Expr,
        span: Span,
    ) -> EvalResult<Value> {
        let is_inf = |e: &Expr| matches!(&*e.kind, ExprKind::Ident(n) if canonical_name(n) == "∞");
        if !self.opts.float64 {
            // Closed forms for symbolic or infinite bounds.
            let lo_v = self.sym_bound(from);
            let hi_v = self.sym_bound(to);
            let integral =
                |s: &Option<crate::cas::Sym>| s.as_ref().is_some_and(|s| s.as_integer().is_some());
            if !(integral(&lo_v) && integral(&hi_v)) {
                let (Some(a), Some(b)) = (lo_v, hi_v) else {
                    return Err(EvalError::other("invalid bounds", span));
                };
                let var = canonical_name(var).to_string();
                let f = self
                    .symbolic_body(&var, body)
                    .ok_or_else(|| EvalError::other("the body has no symbolic form", body.span))?;
                let r = match op {
                    BigOpKind::Sum => crate::cas::summation(&f, &var, &a, &b),
                    BigOpKind::Product => crate::cas::product(&f, &var, &a, &b),
                };
                return match r {
                    Some(r) => self.sym_result(r, false, span),
                    None => Err(EvalError::unsupported(
                        "no closed form found for this sum/product",
                        span,
                    )),
                };
            }
        } else if is_inf(to) {
            return Err(EvalError::unsupported(
                "infinite sums and products are not available in plots",
                to.span,
            ));
        }
        let lo = self.eval(from)?;
        let hi = self.eval(to)?;
        let lo = self.bigop_bound(lo, from.span)?;
        let hi = self.bigop_bound(hi, to.span)?;
        let (identity, arith) = match op {
            BigOpKind::Sum => (Number::zero(), Arith::Add),
            BigOpKind::Product => (Number::one(), Arith::Mul),
        };
        let mut acc = Value::Number(self.mode_num(identity));
        if hi < lo {
            return Ok(acc);
        }
        let var = canonical_name(var).to_string();
        self.push_scope(Vec::new());
        let r = (|| {
            for k in lo..=hi {
                self.tick(span)?;
                let kv = self.mode_num(Number::from(k));
                self.bind_local(&var, Value::Number(kv));
                let v = self.eval(body)?;
                acc = self.arith(arith, acc, v, span)?;
            }
            Ok(acc)
        })();
        self.pop_scope();
        r
    }

    fn bigop_bound(&self, v: Value, span: Span) -> EvalResult<i64> {
        let n = self.expect_num(v, span)?;
        n.integer_value()
            .ok()
            .and_then(|i| i.to_i64())
            .ok_or_else(|| {
                EvalError::new(
                    ErrorKind::Other,
                    format!("sum/product bounds must be integers, got {n}"),
                    span,
                )
            })
    }
}

const XGK: [f64; 8] = [
    0.991_455_371_120_812_6,
    0.949_107_912_342_758_5,
    0.864_864_423_359_769_1,
    0.741_531_185_599_394_4,
    0.586_087_235_467_691_1,
    0.405_845_151_377_397_2,
    0.207_784_955_007_898_5,
    0.0,
];
const WGK: [f64; 8] = [
    0.022_935_322_010_529_22,
    0.063_092_092_629_978_55,
    0.104_790_010_322_250_2,
    0.140_653_259_715_525_9,
    0.169_004_726_639_267_9,
    0.190_350_578_064_785_4,
    0.204_432_940_075_298_9,
    0.209_482_141_084_727_8,
];
const WG: [f64; 4] = [
    0.129_484_966_168_869_7,
    0.279_705_391_489_276_7,
    0.381_830_050_505_118_9,
    0.417_959_183_673_469_4,
];

/// One Gauss–Kronrod 7/15 panel: (integral estimate, error estimate).
fn gk15<E>(
    ev: &mut E,
    f: &mut dyn FnMut(&mut E, f64) -> EvalResult<f64>,
    a: f64,
    b: f64,
) -> EvalResult<(f64, f64)> {
    let c = 0.5 * (a + b);
    let h = 0.5 * (b - a);
    let fc = f(ev, c)?;
    let mut kron = WGK[7] * fc;
    let mut gauss = WG[3] * fc;
    for j in 0..7 {
        let dx = h * XGK[j];
        let s = f(ev, c - dx)? + f(ev, c + dx)?;
        kron += WGK[j] * s;
        if j % 2 == 1 {
            gauss += WG[j / 2] * s;
        }
    }
    Ok((kron * h, ((kron - gauss) * h).abs()))
}

/// Adaptive Gauss–Kronrod quadrature of a plain `f64` function on
/// `[a, b]` (bounds may be infinite); `None` if it does not converge or
/// the integrand is not finite.
pub(crate) fn quad(f: &dyn Fn(f64) -> f64, a: f64, b: f64) -> Option<f64> {
    if a == b {
        return Some(0.0);
    }
    if a > b {
        return quad(f, b, a).map(|v| -v);
    }
    type Map = Box<dyn Fn(f64) -> (f64, f64)>;
    let (ta, tb, map): (f64, f64, Map) = match (a.is_finite(), b.is_finite()) {
        (true, true) => (a, b, Box::new(|t| (t, 1.0))),
        (true, false) => (
            0.0,
            1.0,
            Box::new(move |t: f64| (a + t / (1.0 - t), 1.0 / ((1.0 - t) * (1.0 - t)))),
        ),
        (false, true) => (
            0.0,
            1.0,
            Box::new(move |t: f64| (b - t / (1.0 - t), 1.0 / ((1.0 - t) * (1.0 - t)))),
        ),
        (false, false) => (
            -1.0,
            1.0,
            Box::new(|t: f64| {
                let d = 1.0 - t * t;
                (t / d, (1.0 + t * t) / (d * d))
            }),
        ),
    };
    let mut g = |_: &mut (), t: f64| -> EvalResult<f64> {
        let (x, jac) = map(t);
        let y = f(x);
        if !y.is_finite() {
            return Err(EvalError::other("not finite", Span::default()));
        }
        Ok(y * jac)
    };
    let mut unit = ();
    let first = gk15(&mut unit, &mut g, ta, tb).ok()?;
    let mut parts = vec![(ta, tb, first.0, first.1)];
    for _ in 0..400 {
        let total: f64 = parts.iter().map(|p| p.2).sum();
        let err: f64 = parts.iter().map(|p| p.3).sum();
        if err <= 1e-11_f64.max(1e-11 * total.abs()) {
            return Some(total);
        }
        let (k, _) = parts
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.3.total_cmp(&y.1.3))?;
        let (lo, hi, _, _) = parts.swap_remove(k);
        let mid = 0.5 * (lo + hi);
        let l = gk15(&mut unit, &mut g, lo, mid).ok()?;
        let r = gk15(&mut unit, &mut g, mid, hi).ok()?;
        parts.push((lo, mid, l.0, l.1));
        parts.push((mid, hi, r.0, r.1));
    }
    let total: f64 = parts.iter().map(|p| p.2).sum();
    let err: f64 = parts.iter().map(|p| p.3).sum();
    (err <= 1e-6 * total.abs().max(1.0)).then_some(total)
}
