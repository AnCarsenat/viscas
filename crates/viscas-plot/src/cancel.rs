//! Cooperative cancellation and evaluation budgets for sampling running on
//! worker threads.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared cancellation flag. Cloning shares the same flag, so the UI thread
/// keeps one clone and calls [`CancelToken::cancel`] when the view changes.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Option<Arc<AtomicBool>>);

impl CancelToken {
    /// A fresh, not-yet-cancelled token.
    pub fn new() -> Self {
        Self(Some(Arc::new(AtomicBool::new(false))))
    }

    /// A token that can never be cancelled (cheapest).
    pub fn none() -> Self {
        Self(None)
    }

    pub fn cancel(&self) {
        if let Some(f) = &self.0 {
            f.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.as_ref().is_some_and(|f| f.load(Ordering::Relaxed))
    }

    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

impl From<Arc<AtomicBool>> for CancelToken {
    fn from(flag: Arc<AtomicBool>) -> Self {
        Self(Some(flag))
    }
}

/// Returned when sampling was aborted through a [`CancelToken`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("plot sampling cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Result of a sampler whose work may have been cut short by the
/// evaluation budget (`PlotOptions::max_evals`). Dereferences to the data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sampled<T> {
    pub data: T,
    /// The evaluation budget ran out, so the result is coarser or
    /// incomplete. Cancellation is reported as `Err(Cancelled)` instead.
    pub truncated: bool,
}

impl<T> std::ops::Deref for Sampled<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

impl<'a, T> IntoIterator for &'a Sampled<Vec<T>> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.data.iter()
    }
}

impl<T> Sampled<T> {
    pub fn into_inner(self) -> T {
        self.data
    }
}

/// Evaluation counter shared by the samplers. It is a hard cap: once `max`
/// evaluations were granted, [`Budget::take`] refuses and records the
/// truncation. The cancel flag is polled on every request (one relaxed
/// atomic load, negligible next to a function evaluation).
pub(crate) struct Budget<'a> {
    pub evals: usize,
    max: usize,
    cancel: &'a CancelToken,
    truncated: bool,
}

impl<'a> Budget<'a> {
    pub fn new(max: usize, cancel: &'a CancelToken) -> Self {
        Self {
            evals: 0,
            max,
            cancel,
            truncated: false,
        }
    }

    /// Ask for one evaluation: `Ok(true)` if granted, `Ok(false)` if the
    /// budget is exhausted, `Err` if cancelled.
    #[inline]
    pub fn take(&mut self) -> Result<bool, Cancelled> {
        self.cancel.check()?;
        if self.evals >= self.max {
            self.truncated = true;
            return Ok(false);
        }
        self.evals += 1;
        Ok(true)
    }

    pub fn exhausted(&self) -> bool {
        self.evals >= self.max
    }

    pub fn remaining(&self) -> usize {
        self.max.saturating_sub(self.evals)
    }

    pub fn limit(&self) -> usize {
        self.max
    }

    /// Change the cap (used to give each curve of a list a fair share).
    pub fn set_limit(&mut self, max: usize) {
        self.max = max;
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn reset_truncated(&mut self) {
        self.truncated = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_shares_flag() {
        let t = CancelToken::new();
        let t2 = t.clone();
        assert!(!t2.is_cancelled());
        t.cancel();
        assert!(t2.is_cancelled());
        assert_eq!(t2.check(), Err(Cancelled));
        assert!(!CancelToken::none().is_cancelled());
    }

    #[test]
    fn budget_checks_cancel_on_every_take() {
        let t = CancelToken::new();
        let mut b = Budget::new(10, &t);
        assert_eq!(b.take(), Ok(true));
        t.cancel();
        assert_eq!(b.take(), Err(Cancelled));
    }

    #[test]
    fn budget_is_a_hard_cap() {
        let t = CancelToken::none();
        let mut b = Budget::new(3, &t);
        let granted = (0..10).filter(|_| b.take().unwrap()).count();
        assert_eq!(granted, 3);
        assert_eq!(b.evals, 3);
        assert!(b.exhausted() && b.truncated());
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn sampled_derefs() {
        let s = Sampled {
            data: vec![1, 2],
            truncated: false,
        };
        assert_eq!(s.len(), 2);
        assert_eq!(s.into_inner(), vec![1, 2]);
    }
}
