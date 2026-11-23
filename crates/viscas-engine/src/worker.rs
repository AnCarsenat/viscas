//! Background work (plot rendering, feature analysis) on a small pool of
//! std threads.
//!
//! Tasks carry the [`CancelToken`] of the job they belong to. The queue is
//! a mutex-protected deque: submitting drops every queued task whose job
//! was cancelled (so superseded jobs never pile up), workers skip
//! cancelled tasks, and shutdown stops workers before their next task.
//! Results travel on `mpsc` channels the UI thread drains without blocking
//! — no async runtime, no glib.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

use viscas_plot::CancelToken;

use crate::plot::{PlotLayer, ViewportId};

/// A finished node of a plot job.
#[derive(Clone, Debug)]
pub struct PlotResult {
    pub viewport: ViewportId,
    pub generation: u64,
    /// Revision of the node the layer was compiled at.
    pub revision: u64,
    pub view: viscas_plot::View,
    pub layer: PlotLayer,
}

type Task = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Queue {
    tasks: VecDeque<(CancelToken, Task)>,
    shutdown: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

/// Fixed-size worker pool. Dropping it stops and joins the workers
/// (cancel running jobs first so a slow sample ends promptly).
pub struct PlotPool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl PlotPool {
    /// A pool of `threads` workers (at least one).
    pub fn new(threads: usize) -> PlotPool {
        let shared = Arc::new(Shared::default());
        let workers = (0..threads.max(1))
            .map(|i| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("viscas-plot-{i}"))
                    .stack_size(8 << 20)
                    .spawn(move || worker(&shared))
                    .expect("spawn plot worker")
            })
            .collect();
        PlotPool { shared, workers }
    }

    /// Default size: available cores minus one (for the UI), 1 to 4.
    pub fn default_threads() -> usize {
        std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .saturating_sub(1)
            .clamp(1, 4)
    }

    pub fn threads(&self) -> usize {
        self.workers.len()
    }

    /// Queue `task` unless `cancel` is already set; queued tasks of
    /// cancelled jobs are dropped.
    pub fn spawn(&self, cancel: CancelToken, task: impl FnOnce() + Send + 'static) {
        let mut q = self.lock();
        q.tasks.retain(|(c, _)| !c.is_cancelled());
        if q.shutdown || cancel.is_cancelled() {
            return;
        }
        q.tasks.push_back((cancel, Box::new(task)));
        drop(q);
        self.shared.wake.notify_one();
    }

    /// Tasks waiting in the queue.
    pub fn queued(&self) -> usize {
        self.lock().tasks.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for PlotPool {
    fn drop(&mut self) {
        {
            let mut q = self.lock();
            q.shutdown = true;
            q.tasks.clear();
        }
        self.shared.wake.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(shared: &Shared) {
    loop {
        let task = {
            let mut q = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(t) = q.tasks.pop_front() {
                    break t;
                }
                q = shared.wake.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let (cancel, task) = task;
        if cancel.is_cancelled() {
            continue;
        }
        // Tasks report their own panics; this only keeps the worker alive.
        let _ = catch_unwind(AssertUnwindSafe(task));
    }
}
