//! Where deferred work runs.
//!
//! Some work must not hold up a response (generating an embedding takes hundreds of
//! milliseconds and the write is already durable without it), so it is handed off. This is the
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
/// task differently, and that is the right time to change this, not before there is one.
pub type Task = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Runs deferred work.
pub trait Queue: Send + Sync {
    /// Accepts a task. Returns as soon as the task is accepted, not when it completes: the
    /// caller is a request handler that has already answered.
    fn enqueue(&self, task: Task);

    /// Waits for accepted work to finish, up to `timeout`.
    ///
    /// Called during shutdown. Dropping an accepted task would leave an entity written but
    /// never embedded: invisible to search until someone runs a resync, with nothing to say
    /// it happened.
    ///
    /// Returns whether it finished. `()` would leave a caller unable to tell an empty queue from
    /// an expired timeout. A switchover removing the old queue needs exactly that
    /// distinction, because work still outstanding is work that goes away with it.
    fn drain(
        &self,
        timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = DrainOutcome> + Send + '_>>;
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

    fn drain(
        &self,
        timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = DrainOutcome> + Send + '_>> {
        Box::pin(async move {
            self.tasks.close();
            // A timeout rather than an unbounded wait: a task that hangs must not hold the
            // process open, and the work it was doing is recoverable by a resync.
            match tokio::time::timeout(timeout, self.tasks.wait()).await {
                Ok(()) => DrainOutcome::Finished,
                Err(_) => DrainOutcome::TimedOut,
            }
        })
    }
}

/// Whether a drain ran to completion or ran out of time.
///
/// The difference decides whether the old queue may be removed: work still outstanding when the
/// timeout expires is work that is lost if the queue goes away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainOutcome {
    /// Nothing outstanding. Safe to remove.
    Finished,
    /// The timeout expired first. Something is still running, or something is stuck.
    TimedOut,
}

/// Two queues during an infrastructure switchover (FR-7-3).
///
/// New work goes to the new queue from the moment this is installed; the old one keeps running
/// what it already accepted until it is empty. That is the whole of the switchover: the three
/// stages the spec describes (start sending to the new one, drain the old, remove it) are
/// respectively constructing this, calling [`DrainingQueue::drain_old`], and dropping it.
///
/// It is a `Queue` itself, so nothing upstream knows a switchover is happening. `enqueue` never
/// reaches the old queue: a task sent there during the drain is one more thing to wait for, and
/// the point of the exercise is to reach zero.
///
/// **The two queues are any two `Queue`s.** The spec framed this as needing a second *driver*,
/// but the seam does not care what is behind it: swapping a `LocalQueue` for another
/// `LocalQueue` exercises the same paths, which is what the tests do.
pub struct DrainingQueue {
    new: Arc<dyn Queue>,
    old: Arc<dyn Queue>,
}

impl DrainingQueue {
    pub fn new(new: Arc<dyn Queue>, old: Arc<dyn Queue>) -> Self {
        Self { new, old }
    }

    /// Waits for the old queue's accepted work, up to `timeout`.
    ///
    /// Stage 2. Separate from [`Queue::drain`] because they answer different questions: this one
    /// is "is the old queue finished, so it can be removed", and the other is "is *everything*
    /// finished, so the process can exit". A switchover that called the second would also wait
    /// for work that has only just arrived on the new queue, which is not what it is asking.
    ///
    /// **Returns whether it actually finished.** Stage 3 removes the old queue, which must not
    /// happen while work is still outstanding, so the answer has to reach the caller rather
    /// than being swallowed the way an unreported timeout would be.
    pub async fn drain_old(&self, timeout: std::time::Duration) -> DrainOutcome {
        self.old.drain(timeout).await
    }
}

impl Queue for DrainingQueue {
    fn enqueue(&self, task: Task) {
        self.new.enqueue(task);
    }

    fn drain(
        &self,
        timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = DrainOutcome> + Send + '_>> {
        Box::pin(async move {
            // Both, concurrently rather than one after the other: at shutdown the timeout is a
            // bound on how long the process may take, and spending it twice would double it.
            let (a, b) = tokio::join!(self.new.drain(timeout), self.old.drain(timeout));
            // Finished only if both are: one queue still holding work is one queue too many.
            match (a, b) {
                (DrainOutcome::Finished, DrainOutcome::Finished) => DrainOutcome::Finished,
                _ => DrainOutcome::TimedOut,
            }
        })
    }
}

#[cfg(test)]
#[path = "../../tests/services/queue.rs"]
mod tests;
