use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::services::queue::{DrainOutcome, DrainingQueue, LocalQueue, Queue};

#[tokio::test]
async fn enqueued_work_runs() {
    let queue = LocalQueue::new(4);
    let ran = Arc::new(AtomicUsize::new(0));

    for _ in 0..5 {
        let ran = Arc::clone(&ran);
        queue.enqueue(Box::pin(async move {
            ran.fetch_add(1, Ordering::SeqCst);
        }));
    }

    queue.drain(Duration::from_secs(5)).await;
    assert_eq!(ran.load(Ordering::SeqCst), 5);
}

/// The cap is the point: every task wants a database connection, and without a bound a burst
/// of writes would ask for more than the pool has and starve the requests being served.
#[tokio::test]
async fn concurrency_is_bounded() {
    let queue = LocalQueue::new(2);
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    for _ in 0..10 {
        let in_flight = Arc::clone(&in_flight);
        let peak = Arc::clone(&peak);
        queue.enqueue(Box::pin(async move {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    queue.drain(Duration::from_secs(10)).await;
    assert!(
        peak.load(Ordering::SeqCst) <= 2,
        "at most two at a time, saw {}",
        peak.load(Ordering::SeqCst)
    );
}

/// Draining waits for accepted work. Dropping it would leave an entity written but never
/// embedded — absent from search with nothing to say why.
#[tokio::test]
async fn drain_waits_for_accepted_work() {
    let queue = LocalQueue::new(4);
    let finished = Arc::new(AtomicUsize::new(0));

    let f = Arc::clone(&finished);
    queue.enqueue(Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        f.fetch_add(1, Ordering::SeqCst);
    }));

    queue.drain(Duration::from_secs(5)).await;
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

/// A task that hangs must not hold the process open. The work is recoverable by a resync;
/// an unbounded wait is not recoverable at all.
#[tokio::test]
async fn drain_gives_up_rather_than_hanging() {
    let queue = LocalQueue::new(4);

    queue.enqueue(Box::pin(async {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }));

    let start = std::time::Instant::now();
    queue.drain(Duration::from_millis(100)).await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "drain should time out, took {:?}",
        start.elapsed()
    );
}

/// The switchover's three stages (FR-7-3), in order: new work goes to the new queue, the old
/// one finishes what it already had, and only then is it removable.
#[tokio::test]
async fn a_switchover_sends_new_work_on_and_lets_the_old_queue_finish() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let old = Arc::new(LocalQueue::new(4));
    let new = Arc::new(LocalQueue::new(4));

    let old_ran = Arc::new(AtomicUsize::new(0));
    let new_ran = Arc::new(AtomicUsize::new(0));

    // Stage 0: work already accepted by the old queue, still running when the switch happens.
    let counter = Arc::clone(&old_ran);
    old.enqueue(Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        counter.fetch_add(1, Ordering::SeqCst);
    }));

    // Stage 1: from here, everything goes to the new queue.
    let switching = DrainingQueue::new(
        Arc::clone(&new) as Arc<dyn Queue>,
        Arc::clone(&old) as Arc<dyn Queue>,
    );
    let counter = Arc::clone(&new_ran);
    switching.enqueue(Box::pin(async move {
        counter.fetch_add(1, Ordering::SeqCst);
    }));

    // Stage 2: the old queue's outstanding work completes.
    assert_eq!(
        switching.drain_old(Duration::from_secs(5)).await,
        DrainOutcome::Finished,
        "the old queue emptied within the timeout"
    );
    assert_eq!(
        old_ran.load(Ordering::SeqCst),
        1,
        "the old queue finished what it had accepted"
    );

    switching.drain(Duration::from_secs(5)).await;
    assert_eq!(
        new_ran.load(Ordering::SeqCst),
        1,
        "and the new queue ran what arrived after the switch"
    );
}

/// Nothing sent through the switchover reaches the old queue — otherwise draining it would be
/// chasing a target that keeps moving.
#[tokio::test]
async fn new_work_never_lands_on_the_old_queue() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let old = Arc::new(LocalQueue::new(4));
    let new = Arc::new(LocalQueue::new(4));
    let old_ran = Arc::new(AtomicUsize::new(0));

    let switching = DrainingQueue::new(
        Arc::clone(&new) as Arc<dyn Queue>,
        Arc::clone(&old) as Arc<dyn Queue>,
    );

    let counter = Arc::clone(&old_ran);
    switching.enqueue(Box::pin(async move {
        counter.fetch_add(1, Ordering::SeqCst);
    }));

    assert_eq!(
        switching.drain_old(Duration::from_secs(2)).await,
        DrainOutcome::Finished,
        "an empty queue finishes immediately"
    );
    assert_eq!(
        old_ran.load(Ordering::SeqCst),
        0,
        "the task went to the new queue, so draining the old one saw nothing"
    );
}

/// The distinction stage 3 depends on: a queue that ran out of time is not an empty queue, and
/// removing it would drop the work still running on it.
#[tokio::test]
async fn a_drain_that_runs_out_of_time_says_so() {
    use std::time::Duration;

    let old = Arc::new(LocalQueue::new(4));
    let new = Arc::new(LocalQueue::new(4));

    // Longer than the timeout it will be given.
    old.enqueue(Box::pin(async {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }));

    let switching = DrainingQueue::new(
        Arc::clone(&new) as Arc<dyn Queue>,
        Arc::clone(&old) as Arc<dyn Queue>,
    );

    assert_eq!(
        switching.drain_old(Duration::from_millis(100)).await,
        DrainOutcome::TimedOut,
        "still running, so the old queue must not be removed"
    );
}
