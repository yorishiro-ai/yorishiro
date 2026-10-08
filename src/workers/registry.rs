//! The authoritative list of background workers this process registers with Loco.
//!
//! One registration per worker type is the only place a worker's tags and queue are stated: both come from the type's own `BackgroundWorker::tags()` and `queue()`.
//! Loco registration, the Redis queue configuration and the `worker-tags` command are all derived from the same [`WorkerRegistry`], so none of them can drift from the others.
//!
//! [`WorkerRegistry::community`] is the baseline.
//! An edition adds its own workers by calling [`WorkerRegistry::register`] on it from the composition root, which is the only place that knows which editions exist.

use std::future::Future;
use std::pin::Pin;

use loco_rs::Result;
use loco_rs::app::AppContext;
use loco_rs::bgworker::{BackgroundWorker, Queue};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::workers::embedding_sync::{
    EmbeddingSyncArgs, EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared,
    EmbeddingSyncWorkerTenantPrivate,
};
use crate::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
use crate::workers::reindex::{
    ReindexArgs, ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
};

type ConnectFn =
    for<'a> fn(&'a AppContext, &'a Queue) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

fn connect<'a, A, W>(
    ctx: &'a AppContext,
    queue: &'a Queue,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>
where
    A: Serialize + DeserializeOwned + Send + Sync + 'static,
    W: BackgroundWorker<A> + 'static,
{
    Box::pin(async move { queue.register(W::build(ctx)).await })
}

/// One worker type, with the tags and queue it declares for itself.
struct WorkerRegistration {
    class_name: String,
    tags: Vec<String>,
    queue: Option<String>,
    connect: ConnectFn,
}

/// Every worker type this process registers, in registration order.
#[derive(Default)]
pub struct WorkerRegistry {
    registrations: Vec<WorkerRegistration>,
}

impl WorkerRegistry {
    /// The workers the base application owns: embedding sync and reindex, one type per worker class, and the query embedding worker.
    #[must_use]
    pub fn community() -> Self {
        Self::default()
            .register::<EmbeddingSyncArgs, EmbeddingSyncWorkerTenantPrivate>()
            .register::<EmbeddingSyncArgs, EmbeddingSyncWorkerOfficial>()
            .register::<EmbeddingSyncArgs, EmbeddingSyncWorkerShared>()
            .register::<ReindexArgs, ReindexWorkerTenantPrivate>()
            .register::<ReindexArgs, ReindexWorkerOfficial>()
            .register::<ReindexArgs, ReindexWorkerShared>()
            .register::<QueryEmbeddingArgs, QueryEmbeddingWorker>()
    }

    /// Adds worker type `W`, whose tags and queue are whatever `W` declares.
    ///
    /// # Panics
    /// Panics if `W` is already registered, since Loco keys handlers by class name and a second registration would silently replace the first.
    #[must_use]
    pub fn register<A, W>(mut self) -> Self
    where
        A: Serialize + DeserializeOwned + Send + Sync + 'static,
        W: BackgroundWorker<A> + 'static,
    {
        let class_name = W::class_name();
        assert!(
            self.registrations
                .iter()
                .all(|existing| existing.class_name != class_name),
            "duplicate background worker: {class_name}"
        );
        self.registrations.push(WorkerRegistration {
            class_name,
            tags: W::tags(),
            queue: W::queue(),
            connect: connect::<A, W>,
        });
        self
    }

    /// Every tag a worker process can request through `--worker=...`, each once, in registration order.
    #[must_use]
    pub fn tags(&self) -> Vec<String> {
        distinct(
            self.registrations
                .iter()
                .flat_map(|registration| registration.tags.iter().cloned()),
        )
    }

    /// Every named queue the registered workers enqueue to, each once, in registration order.
    #[must_use]
    pub fn queues(&self) -> Vec<String> {
        distinct(
            self.registrations
                .iter()
                .filter_map(|registration| registration.queue.clone()),
        )
    }

    /// Registers every worker with `queue`, in registration order.
    ///
    /// # Errors
    /// Returns the first registration error Loco reports.
    pub(crate) async fn connect(&self, ctx: &AppContext, queue: &Queue) -> Result<()> {
        for registration in &self.registrations {
            (registration.connect)(ctx, queue).await?;
        }
        Ok(())
    }
}

fn distinct(values: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = Vec::new();
    for value in values {
        if !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}
