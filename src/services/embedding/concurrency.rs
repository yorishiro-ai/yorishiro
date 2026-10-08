//! Process-local admission for embedding provider calls.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Limits concurrent document and query calls made by one worker process.
#[derive(Clone)]
pub(crate) struct EmbeddingConcurrency {
    semaphore: Arc<Semaphore>,
}

impl EmbeddingConcurrency {
    /// Creates a process-local provider admission policy.
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(limit)),
        }
    }

    /// Waits fairly for one provider call slot.
    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, tokio::sync::AcquireError> {
        self.semaphore.clone().acquire_owned().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    use super::EmbeddingConcurrency;

    #[tokio::test]
    async fn capacity_one_admits_in_fifo_order_for_document_and_query_work() {
        let policy = Arc::new(EmbeddingConcurrency::new(1));
        let held = policy.acquire().await.unwrap();
        let (sent, mut received) = mpsc::unbounded_channel();
        let mut tasks = Vec::new();
        for kind in ["document", "query"] {
            let policy = policy.clone();
            let sent = sent.clone();
            tasks.push(tokio::spawn(async move {
                let permit = policy.acquire().await.unwrap();
                sent.send(kind).unwrap();
                drop(permit);
            }));
            tokio::task::yield_now().await;
        }
        drop(sent);
        drop(held);
        let first = received.recv().await.unwrap();
        let second = received.recv().await.unwrap();
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!((first, second), ("document", "query"));
    }

    #[tokio::test]
    async fn cancellation_and_panic_release_permits() {
        let policy = Arc::new(EmbeddingConcurrency::new(1));
        let held = policy.acquire().await.unwrap();
        let cancelled = {
            let policy = policy.clone();
            tokio::spawn(async move { policy.acquire().await })
        };
        cancelled.abort();
        drop(held);

        let panic_policy = policy.clone();
        let panicked = tokio::spawn(async move {
            let _permit = panic_policy.acquire().await.unwrap();
            panic!("provider panic");
        });
        assert!(panicked.await.is_err());
        assert!(
            timeout(Duration::from_secs(1), policy.acquire())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn three_calls_eventually_admit_and_instances_are_process_local() {
        let first = Arc::new(EmbeddingConcurrency::new(1));
        let second = Arc::new(EmbeddingConcurrency::new(1));
        let first_permit = first.acquire().await.unwrap();
        assert!(
            timeout(Duration::from_millis(20), second.acquire())
                .await
                .is_ok()
        );

        let mut calls = Vec::new();
        for _ in 0..3 {
            let policy = first.clone();
            calls.push(tokio::spawn(async move {
                let permit = policy.acquire().await.unwrap();
                tokio::task::yield_now().await;
                drop(permit);
            }));
        }
        drop(first_permit);
        for call in calls {
            timeout(Duration::from_secs(1), call)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
