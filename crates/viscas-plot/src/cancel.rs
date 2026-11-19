//! Cooperative cancellation for sampling running on worker threads.

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

/// Evaluation counter shared by the samplers: enforces the evaluation budget
/// and polls the cancel flag every few evaluations.
pub(crate) struct Budget<'a> {
    pub evals: usize,
    max: usize,
    cancel: &'a CancelToken,
}

impl<'a> Budget<'a> {
    pub fn new(max: usize, cancel: &'a CancelToken) -> Self {
        Self {
            evals: 0,
            max,
            cancel,
        }
    }

    #[inline]
    pub fn tick(&mut self) -> Result<(), Cancelled> {
        self.evals += 1;
        if self.evals & 63 == 0 {
            self.cancel.check()?;
        }
        Ok(())
    }

    pub fn exhausted(&self) -> bool {
        self.evals >= self.max
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
    fn budget_polls_cancel() {
        let t = CancelToken::new();
        t.cancel();
        let mut b = Budget::new(10, &t);
        let r: Result<(), Cancelled> = (0..64).try_for_each(|_| b.tick());
        assert_eq!(r, Err(Cancelled));
        assert!(b.exhausted());
    }
}
