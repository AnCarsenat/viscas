//! Cooperative cancellation of long numeric computations.
//!
//! A worker thread installs a shared flag with [`install_interrupt`]; every
//! long-running loop of this module calls [`check_interrupt`], which fails with
//! [`NumberError::Interrupted`] once the flag is set. The flag is per thread,
//! so concurrent workers can be cancelled independently.
//!
//! ```
//! use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
//! use viscas_core::number::{install_interrupt, Number, NumberError};
//!
//! let flag = Arc::new(AtomicBool::new(false));
//! let _guard = install_interrupt(flag.clone());
//! flag.store(true, Ordering::Relaxed); // e.g. from the UI thread
//! assert_eq!(Number::from(5000).factorial(Default::default()), Err(NumberError::Interrupted));
//! ```

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{NumResult, NumberError};

thread_local! {
    static FLAG: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Restores the previously installed flag (if any) when dropped.
#[must_use = "the interrupt flag is uninstalled when the guard is dropped"]
pub struct InterruptGuard {
    previous: Option<Arc<AtomicBool>>,
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        FLAG.with(|f| *f.borrow_mut() = previous);
    }
}

/// Installs `flag` as the interrupt flag of the current thread until the
/// returned guard is dropped.
pub fn install_interrupt(flag: Arc<AtomicBool>) -> InterruptGuard {
    let previous = FLAG.with(|f| f.borrow_mut().replace(flag));
    InterruptGuard { previous }
}

/// Fails with [`NumberError::Interrupted`] if the current thread's flag is set.
pub fn check_interrupt() -> NumResult<()> {
    let set = FLAG.with(|f| {
        f.borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    });
    if set {
        Err(NumberError::Interrupted)
    } else {
        Ok(())
    }
}
