//! Benchmark-style checks: sample counts at 1200×800 stay within a budget
//! that allows re-sampling every frame at 60 fps. Timings are printed (run
//! with `cargo test --release -p viscas-plot --test perf -- --nocapture`)
//! but only evaluation counts are asserted, so the test is not flaky.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use viscas_plot::*;

type Fn1 = fn(f64) -> f64;
type Fn2 = fn(f64, f64) -> f64;

fn counted1(f: Fn1, view: &View) -> (usize, usize, Duration) {
    let n = AtomicUsize::new(0);
    let g = |x: f64| {
        n.fetch_add(1, Ordering::Relaxed);
        f(x)
    };
    let t = Instant::now();
    let lines = plot_explicit(&g, view, &PlotOptions::default()).unwrap();
    let dt = t.elapsed();
    (n.load(Ordering::Relaxed), lines.len(), dt)
}

fn counted2(f: Fn2, view: &View) -> (usize, usize, Duration) {
    let n = AtomicUsize::new(0);
    let g = |x: f64, y: f64| {
        n.fetch_add(1, Ordering::Relaxed);
        f(x, y)
    };
    let t = Instant::now();
    let lines = plot_implicit(&g, view, &PlotOptions::default()).unwrap();
    let dt = t.elapsed();
    (n.load(Ordering::Relaxed), lines.len(), dt)
}

#[test]
fn explicit_sample_budget() {
    let v = View::default(); // 1200×800, x ∈ [-10, 10]
    let cases: [(&str, Fn1, usize); 7] = [
        ("sin x", f64::sin, 4_000),
        ("x^2", |x| x * x, 4_000),
        ("sqrt x", f64::sqrt, 4_000),
        ("1/x", |x| 1.0 / x, 5_000),
        ("floor x", f64::floor, 5_000),
        ("tan x", f64::tan, 15_000),
        ("e^x", f64::exp, 4_000),
    ];
    for (name, f, max) in cases {
        let (evals, lines, dt) = counted1(f, &v);
        println!("{name:>8}: {evals:>6} evals, {lines:>2} polylines, {dt:?}");
        assert!(evals <= max, "{name}: {evals} evals > {max}");
        assert!(lines >= 1);
    }
}

#[test]
fn implicit_sample_budget() {
    let v = View::default();
    let cases: [(&str, Fn2, usize); 4] = [
        ("circle", |x, y| x * x + y * y - 9.0, 50_000),
        ("hyperbola", |x, y| x * x - y * y - 1.0, 60_000),
        ("1/(xy)=1", |x, y| 1.0 / (x * y) - 1.0, 60_000),
        ("sin x + sin y", |x, y| x.sin() + y.sin() - 0.3, 130_000),
    ];
    for (name, f, max) in cases {
        let (evals, lines, dt) = counted2(f, &v);
        println!("{name:>13}: {evals:>6} evals, {lines:>2} polylines, {dt:?}");
        assert!(evals <= max, "{name}: {evals} evals > {max}");
        assert!(lines >= 1);
    }
}

#[test]
fn cancel_from_another_thread() {
    let token = CancelToken::new();
    let opts = PlotOptions::with_cancel(token.clone());
    let started = Arc::new(AtomicUsize::new(0));
    let s2 = started.clone();
    // A deliberately slow function so the cancel lands mid-sampling.
    let f = move |x: f64, y: f64| {
        s2.fetch_add(1, Ordering::Relaxed);
        std::thread::sleep(Duration::from_micros(20));
        x * x + y * y - 1.0
    };
    let worker = std::thread::spawn(move || plot_implicit(&f, &View::default(), &opts));
    while started.load(Ordering::Relaxed) < 100 {
        std::thread::yield_now();
    }
    token.cancel();
    assert_eq!(worker.join().unwrap(), Err(Cancelled));
    // Cancellation is polled every 64 evaluations.
    assert!(started.load(Ordering::Relaxed) < 5_000);
}
