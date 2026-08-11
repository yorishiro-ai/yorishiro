use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::services::queue::{LocalQueue, Queue};

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
