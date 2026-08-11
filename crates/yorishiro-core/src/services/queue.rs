//! Where deferred work runs.
//!
//! Some work must not hold up a response — generating an embedding takes hundreds of
//! milliseconds and the write is already durable without it — so it is handed off. This is the
//! seam between deciding to defer something and deciding where it runs.
//!
//! In one process that is a task on the runtime, bounded by a semaphore and awaited at
//! shutdown. Across several it is a queue with an acknowledgement, so a task survives the
//! worker that picked it up. The caller says what to run and how many may run at once; it does
//! not say which of those is happening.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio_util::task::TaskTracker;

/// Deferred work, already carrying everything it needs.
///
/// A boxed future rather than a serialisable payload: a payload is what a distributed queue
/// requires, and demanding one here would impose that cost on the in-process case that does
/// not need it. A driver that ships work between machines will need the caller to describe the
/// task differently, and that is the right time to change this — not before there is one.
pub type Task = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Runs deferred work.
pub trait Queue: Send + Sync {
    /// Accepts a task. Returns as soon as the task is accepted, not when it completes — the
    /// caller is a request handler that has already answered.
    fn enqueue(&self, task: Task);

    /// Waits for accepted work to finish, up to `timeout`.
    ///
    /// Called during shutdown. Dropping an accepted task would leave an entity written but
    /// never embedded — invisible to search until someone runs a resync, with nothing to say
    /// it happened.
    fn drain(&self, timeout: std::time::Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Runs tasks on the current process's runtime, at most `concurrency` at a time.
///
/// The bound is what keeps a burst of writes from exhausting the connection pool: every task
/// wants a connection, and without a cap a thousand writes would ask for a thousand at once
/// and starve the requests being served.
pub struct LocalQueue {
    tasks: TaskTracker,
    permits: Arc<Semaphore>,
}

impl LocalQueue {
    pub fn new(concurrency: usize) -> Self {
        Self {
            tasks: TaskTracker::new(),
            permits: Arc::new(Semaphore::new(concurrency)),
        }
    }

    /// The tracker, for a caller that needs to await tasks directly (tests, mostly).
    pub fn tracker(&self) -> &TaskTracker {
        &self.tasks
    }
}

impl Queue for LocalQueue {
    fn enqueue(&self, task: Task) {
        let permits = Arc::clone(&self.permits);
        self.tasks.spawn(async move {
            // The permit is taken inside the task, not before spawning: taking it first would
            // block the request handler that is trying to hand the work off.
            let Ok(_permit) = permits.acquire_owned().await else {
                // Unreachable while the semaphore is open, which it always is.
                return;
            };
            task.await;
        });
    }

    fn drain(&self, timeout: std::time::Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.tasks.close();
            // A timeout rather than an unbounded wait: a task that hangs must not hold the
            // process open, and the work it was doing is recoverable by a resync.
            let _ = tokio::time::timeout(timeout, self.tasks.wait()).await;
        })
    }
}

#[cfg(test)]
#[path = "../../tests/services/queue.rs"]
mod tests;
