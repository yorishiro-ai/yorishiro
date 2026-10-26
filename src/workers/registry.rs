//! The authoritative list of background workers this process registers with Loco.
//!
//! A worker type states no tags and no queue: which workers receive a job is the job's [`crate::workers::target::WorkerTarget`], chosen when it is enqueued, and which targets a process serves is chosen when it starts.
//! The registry only says which handlers a process can run, by Loco's class name.
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

use crate::workers::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorker};
use crate::workers::fill_defaults::{FillDefaultsArgs, FillDefaultsWorker};
use crate::workers::query_embedding::{QueryEmbeddingArgs, QueryEmbeddingWorker};
use crate::workers::reindex::{ReindexArgs, ReindexWorker};
use crate::workers::undo::{UndoArgs, UndoWorker};

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

/// One worker type, by the class name Loco keys its handler under.
struct WorkerRegistration {
    class_name: String,
    connect: ConnectFn,
}

/// Every worker type this process registers, in registration order.
#[derive(Default)]
pub struct WorkerRegistry {
    registrations: Vec<WorkerRegistration>,
}

impl WorkerRegistry {
    /// The workers the base application owns, including durable migration operations.
    #[must_use]
    pub fn community() -> Self {
        Self::default()
            .register::<EmbeddingSyncArgs, EmbeddingSyncWorker>()
            .register::<ReindexArgs, ReindexWorker>()
            .register::<QueryEmbeddingArgs, QueryEmbeddingWorker>()
            .register::<FillDefaultsArgs, FillDefaultsWorker>()
            .register::<UndoArgs, UndoWorker>()
            .register::<super::import::ImportArgs, super::import::ImportWorker>()
    }

    /// Adds worker type `W`.
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
            connect: connect::<A, W>,
        });
        self
    }

    /// Registers only this process role's handlers, in registration order.
    ///
    /// # Errors
    /// Returns the first registration error Loco reports.
    pub(crate) async fn connect(&self, ctx: &AppContext, queue: &Queue) -> Result<()> {
        let role = ctx
            .config
            .settings::<crate::data::settings::Settings>()?
            .consumer_role;
        for registration in &self.registrations {
            let worker_role = if is_embedding_worker(&registration.class_name) {
                crate::data::settings::ConsumerRole::Embedding
            } else {
                crate::data::settings::ConsumerRole::Jobs
            };
            if worker_role != role {
                continue;
            }
            (registration.connect)(ctx, queue).await?;
        }
        Ok(())
    }
}

pub(crate) fn is_embedding_worker(name: &str) -> bool {
    [
        EmbeddingSyncWorker::class_name(),
        ReindexWorker::class_name(),
        QueryEmbeddingWorker::class_name(),
    ]
    .iter()
    .any(|embedding| embedding == name)
}
