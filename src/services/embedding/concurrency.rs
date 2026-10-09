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
