//! Integer algorithms: factorials, binomials, primality and factorization.

use num_bigint::{BigInt, BigUint};
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};

use super::{NumResult, NumberError};

/// Exact results larger than this many bits are refused with
/// [`NumberError::Overflow`] (≈ 20 million decimal digits).
pub(crate) const MAX_EXACT_BITS: u64 = 1 << 26;

/// Largest argument accepted by [`factorial`].
pub const MAX_FACTORIAL: u64 = 200_000;

/// Product of the integers in `lo..=hi` by binary splitting.
fn range_product(lo: u64, hi: u64) -> BigUint {
    if lo > hi {
        return BigUint::one();
    }
    if hi - lo < 16 {
        let mut p = BigUint::one();
        for k in lo..=hi {
            p *= k;
        }
        return p;
    }
    let mid = lo + (hi - lo) / 2;
    range_product(lo, mid) * range_product(mid + 1, hi)
}

/// `n!` for `0 <= n <= MAX_FACTORIAL`.
pub fn factorial(n: &BigInt) -> NumResult<BigInt> {
    if n.is_negative() {
        return Err(NumberError::Domain("factorial of a negative integer"));
    }
    let n = n.to_u64().filter(|&n| n <= MAX_FACTORIAL);
    let n = n.ok_or(NumberError::Overflow)?;
    Ok(BigInt::from(range_product(2, n)))
}

/// Binomial coefficient `C(n, k)` for integers. Extended to negative `n` by
/// `C(n, k) = (-1)^k · C(k - n - 1, k)`; zero when `k < 0` or `0 <= n < k`.
pub fn n_choose_k(n: &BigInt, k: &BigInt) -> NumResult<BigInt> {
    if k.is_negative() {
        return Ok(BigInt::zero());
    }
    if n.is_negative() {
        let m = k - n - 1u32;
        let c = n_choose_k(&m, k)?;
        return Ok(if k.is_odd() { -c } else { c });
    }
    if k > n {
        return Ok(BigInt::zero());
    }
    let k = std::cmp::min(k.clone(), n - k);
    let k = k.to_u64().ok_or(NumberError::Overflow)?;
    if k > MAX_FACTORIAL * 4 {
        return Err(NumberError::Overflow);
    }
    let mut acc = BigInt::one();
    for i in 0..k {
        acc = acc * (n - i) / (i + 1);
    }
    Ok(acc)
}

/// Permutations `P(n, k) = n! / (n - k)!` for `n, k >= 0`; zero when `k > n`.
pub fn n_permute_k(n: &BigInt, k: &BigInt) -> NumResult<BigInt> {
    if n.is_negative() || k.is_negative() {
        return Err(NumberError::Domain("nPr of a negative integer"));
    }
    if k > n {
        return Ok(BigInt::zero());
    }
    let k = k.to_u64().ok_or(NumberError::Overflow)?;
    if k > MAX_FACTORIAL * 4 {
        return Err(NumberError::Overflow);
    }
    let mut acc = BigInt::one();
    for i in 0..k {
        acc *= n - i;
    }
    Ok(acc)
}

const SMALL_PRIMES: [u64; 13] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41];

/// The first 13 primes as Miller–Rabin bases are deterministic below this bound
/// (ψ₁₃ = 3317044064679887385961981).
const DETERMINISTIC_BOUND: &str = "3317044064679887385961981";

fn mul_mod(a: u64, b: u64, m: u64) -> u64 {
    ((a as u128 * b as u128) % m as u128) as u64
}

fn pow_mod(mut b: u64, mut e: u64, m: u64) -> u64 {
    let mut r = 1 % m;
    b %= m;
    while e > 0 {
        if e & 1 == 1 {
            r = mul_mod(r, b, m);
        }
        b = mul_mod(b, b, m);
        e >>= 1;
    }
    r
}

/// Deterministic Miller–Rabin for `u64`.
fn is_prime_u64(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    for &p in &SMALL_PRIMES {
        if n.is_multiple_of(p) {
            return n == p;
        }
    }
    let s = (n - 1).trailing_zeros();
    let d = (n - 1) >> s;
    'bases: for &a in &SMALL_PRIMES[..12] {
        let mut x = pow_mod(a, d, n);
        if x == 1 || x == n - 1 {
            continue;
        }
        for _ in 1..s {
            x = mul_mod(x, x, n);
            if x == n - 1 {
                continue 'bases;
            }
        }
        return false;
    }
    true
}

fn miller_rabin_big(n: &BigUint, a: &BigUint) -> bool {
    let one = BigUint::one();
    let n1 = n - &one;
    let s = n1.trailing_zeros().unwrap_or(0);
    let d = &n1 >> s;
    let mut x = a.modpow(&d, n);
    if x == one || x == n1 {
        return true;
    }
    for _ in 1..s {
        x = (&x * &x) % n;
        if x == n1 {
            return true;
        }
    }
    false
}

/// Primality test.
///
/// Deterministic for `n < 3.3·10²⁴` (which covers every `u64`), using the first
/// 13 primes as Miller–Rabin bases. Larger numbers additionally get 24
/// pseudo-random bases, so a composite passes with probability below 4⁻³⁷.
pub fn is_prime(n: &BigInt) -> bool {
    if n.is_negative() {
        return false;
    }
    if let Some(small) = n.to_u64() {
        return is_prime_u64(small);
    }
    let n = n.magnitude();
    for p in trial_primes() {
        if (n % p).is_zero() {
            return false;
        }
    }
    let bound: BigUint = DETERMINISTIC_BOUND.parse().expect("valid constant");
    for &a in &SMALL_PRIMES {
        if !miller_rabin_big(n, &BigUint::from(a)) {
            return false;
        }
    }
    if n < &bound {
        return true;
    }
    let mut rng = SplitMix64(0x5eed_cafe_f00d_d00d ^ n.bits());
    let n3 = n - 3u32;
    for _ in 0..24 {
        let a = random_below(&mut rng, &n3) + 2u32;
        if !miller_rabin_big(n, &a) {
            return false;
        }
    }
    true
}

/// Small deterministic PRNG (SplitMix64) for Miller–Rabin bases and Pollard rho.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

fn random_below(rng: &mut SplitMix64, n: &BigUint) -> BigUint {
    let words = n.bits().div_ceil(64) as usize + 1;
    let digits: Vec<u64> = (0..words).map(|_| rng.next()).collect();
    let mut r = BigUint::zero();
    for d in digits {
        r = (r << 64usize) + d;
    }
    r % n
}

fn trial_primes() -> impl Iterator<Item = u32> {
    (2u32..1000).filter(|&k| is_prime_u64(k as u64))
}

/// Pollard–Brent rho on `u64`; returns a non-trivial factor of an odd
/// composite `n` that is not a perfect square.
fn rho_u64(n: u64, rng: &mut SplitMix64) -> u64 {
    const BATCH: u64 = 128;
    if n.is_multiple_of(2) {
        return 2;
    }
    let f = |x: u64, c: u64| ((x as u128 * x as u128 + c as u128) % n as u128) as u64;
    loop {
        let c = rng.next() % (n - 1) + 1;
        let mut y = rng.next() % n;
        let (mut r, mut q, mut g) = (1u64, 1u64, 1u64);
        let mut x;
        let mut ys;
        loop {
            x = y;
            for _ in 0..r {
                y = f(y, c);
            }
            let mut k = 0;
            ys = y;
            while k < r && g == 1 {
                ys = y;
                for _ in 0..BATCH.min(r - k) {
                    y = f(y, c);
                    q = mul_mod(q, x.abs_diff(y), n);
                }
                g = q.gcd(&n);
                k += BATCH;
            }
            r *= 2;
            if g != 1 {
                break;
            }
        }
        if g == n {
            loop {
                ys = f(ys, c);
                g = x.abs_diff(ys).gcd(&n);
                if g > 1 {
                    break;
                }
            }
        }
        if g != n {
            return g;
        }
    }
}

/// Pollard rho (Floyd, batched gcd) on big integers.
fn rho_big(n: &BigUint, rng: &mut SplitMix64) -> BigUint {
    let one = BigUint::one();
    loop {
        let c = random_below(rng, n) + 1u32;
        let f = |x: &BigUint| (x * x + &c) % n;
        let mut x = random_below(rng, n);
        let mut y = x.clone();
        let mut g = one.clone();
        while g.is_one() {
            let mut q = one.clone();
            let mut saved = (x.clone(), y.clone());
            for _ in 0..64 {
                x = f(&x);
                y = f(&f(&y));
                let d = if x > y { &x - &y } else { &y - &x };
                q = (q * d) % n;
            }
            g = q.gcd(n);
            if g == *n {
                // Batch overshot: redo one step at a time.
                g = one.clone();
                for _ in 0..64 {
                    saved.0 = f(&saved.0);
                    saved.1 = f(&f(&saved.1));
                    let d = if saved.0 > saved.1 {
                        &saved.0 - &saved.1
                    } else {
                        &saved.1 - &saved.0
                    };
                    g = d.gcd(n);
                    if !g.is_one() {
                        break;
                    }
                }
                break;
            }
        }
        if !g.is_one() && g != *n {
            return g;
        }
    }
}

fn factor_into(n: BigUint, rng: &mut SplitMix64, out: &mut Vec<BigUint>) {
    if n.is_one() {
        return;
    }
    if is_prime(&BigInt::from(n.clone())) {
        out.push(n);
        return;
    }
    let root = n.sqrt();
    if &root * &root == n {
        factor_into(root.clone(), rng, out);
        factor_into(root, rng, out);
        return;
    }
    let d = match n.to_u64() {
        Some(small) => BigUint::from(rho_u64(small, rng)),
        None => rho_big(&n, rng),
    };
    let rest = &n / &d;
    factor_into(d, rng, out);
    factor_into(rest, rng, out);
}

/// Prime factorization of a non-zero integer as ascending `(prime, exponent)`
/// pairs; negative numbers get a leading `(-1, 1)`; `1` gives an empty list.
///
/// Trial division by primes below 1000, then Pollard rho (Brent variant for
/// `u64`). Practical for numbers whose second-largest prime factor has up to
/// ~20 digits.
pub fn factorize(n: &BigInt) -> NumResult<Vec<(BigInt, u32)>> {
    if n.is_zero() {
        return Err(NumberError::Domain("factorization of 0"));
    }
    let mut result: Vec<(BigInt, u32)> = Vec::new();
    if n.is_negative() {
        result.push((BigInt::from(-1), 1));
    }
    let mut m = n.magnitude().clone();
    for p in trial_primes() {
        let mut k = 0;
        while (&m % p).is_zero() {
            m /= p;
            k += 1;
        }
        if k > 0 {
            result.push((BigInt::from(p), k));
        }
    }
    let mut primes = Vec::new();
    let mut rng = SplitMix64(0x0123_4567_89ab_cdef);
    factor_into(m, &mut rng, &mut primes);
    primes.sort();
    for p in primes {
        let p = BigInt::from(p);
        match result.last_mut() {
            Some((q, k)) if *q == p => *k += 1,
            _ => result.push((p, 1)),
        }
    }
    Ok(result)
}
