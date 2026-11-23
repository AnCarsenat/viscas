//! Background plot rendering on a small pool of std threads.
//!
//! Jobs are split into one task per node so a slow implicit plot does not
//! hold up the cheap curves of the same viewport, and results stream back
//! as they finish. Workers share one task queue (an `mpsc` receiver behind
//! a mutex) and send results on an `mpsc` channel the UI thread drains
//! with [`PlotPool::try_recv`] — no async runtime, no glib.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::plot::{PlotJob, PlotLayer, ViewportId};

/// A finished node of a job.
#[derive(Clone, Debug)]
pub struct PlotResult {
    pub viewport: ViewportId,
    pub generation: u64,
    /// Revision of the node the layer was compiled at.
    pub revision: u64,
    pub view: viscas_plot::View,
    pub layer: PlotLayer,
}

struct Task {
    job: Arc<PlotJob>,
    index: usize,
}

/// Fixed-size worker pool. Dropping it stops and joins the workers.
pub struct PlotPool {
    tasks: Option<Sender<Task>>,
    results: Receiver<PlotResult>,
    workers: Vec<JoinHandle<()>>,
}

impl PlotPool {
    /// A pool of `threads` workers (at least one).
    pub fn new(threads: usize) -> PlotPool {
        let (task_tx, task_rx) = mpsc::channel::<Task>();
        let (res_tx, res_rx) = mpsc::channel::<PlotResult>();
        let task_rx = Arc::new(Mutex::new(task_rx));
        let workers = (0..threads.max(1))
            .map(|i| {
                let rx = task_rx.clone();
                let tx = res_tx.clone();
                std::thread::Builder::new()
                    .name(format!("viscas-plot-{i}"))
                    .stack_size(8 << 20)
                    .spawn(move || worker(&rx, &tx))
                    .expect("spawn plot worker")
            })
            .collect();
        PlotPool {
            tasks: Some(task_tx),
            results: res_rx,
            workers,
        }
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

    /// Queue every item of `job`.
    pub fn submit(&self, job: PlotJob) {
        let job = Arc::new(job);
        if let Some(tx) = &self.tasks {
            for index in 0..job.items.len() {
                // Workers only stop when the pool is dropped.
                let _ = tx.send(Task {
                    job: job.clone(),
                    index,
                });
            }
        }
    }

    /// Next finished result, if any (never blocks).
    pub fn try_recv(&self) -> Option<PlotResult> {
        self.results.try_recv().ok()
    }

    /// Wait up to `timeout` for the next result (tests, headless export).
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<PlotResult> {
        self.results.recv_timeout(timeout).ok()
    }
}

impl Drop for PlotPool {
    fn drop(&mut self) {
        self.tasks = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(tasks: &Mutex<Receiver<Task>>, results: &Sender<PlotResult>) {
    loop {
        let task = {
            let Ok(rx) = tasks.lock() else { return };
            match rx.recv() {
                Ok(t) => t,
                Err(_) => return,
            }
        };
        let job = &task.job;
        if job.cancel.is_cancelled() {
            continue;
        }
        let item = &job.items[task.index];
        let layer = match catch_unwind(AssertUnwindSafe(|| job.run_item(task.index))) {
            Ok(Ok(layer)) => layer,
            // Cancelled: a newer job supersedes this one.
            Ok(Err(_)) => continue,
            Err(_) => PlotLayer {
                node_id: item.node,
                style: item.style.clone(),
                primitives: Vec::new(),
                truncated: false,
                error: Some("plotting failed (internal error)".into()),
                stale: false,
            },
        };
        if job.cancel.is_cancelled() {
            continue;
        }
        let r = PlotResult {
            viewport: job.viewport,
            generation: job.generation,
            revision: item.revision,
            view: job.view,
            layer,
        };
        if results.send(r).is_err() {
            return;
        }
    }
}
