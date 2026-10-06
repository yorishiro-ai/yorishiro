//! Generates and stores an entity's embedding vector via Loco's persistent background queue.
//!
//! A queue provider rather than a bare `tokio::spawn`: a spawned task loses every in-flight sync on a process restart, a forced kill, or a provider outage past its own retry budget, leaving the entity's `embedding` column permanently `NULL` with nothing to retry it (`tasks::resync_embeddings` is the operational recovery command for rows in that state).
//! The configured Loco queue provider persists the job, so a re-deployed or restarted process resumes it instead of losing it.
//!
//! **There is no "subscribe to every `WorkerClass`" worker mode.** An empty tag list makes loco's
//! dequeue query `AND (tags IS NULL)` (confirmed against the pinned `loco-rs` 1.2.0 `bgworker/pg.rs`, and the
//! matching logic in `sqlt.rs`/`redis.rs`), so a bare `--worker` process dequeues only *untagged*
//! jobs. Every job this module enqueues carries exactly one tag, so such a process takes none of
//! them, rather than taking "the leftover ones nothing else claimed".
//!
//! Covering every class in one process can use `yorishiro worker-tags` output piped into
//! `--worker=...`, or the `worker-wrapper.sh` script that does it automatically at boot.
//! This all-job command names every currently registered tag, whichever edition registered it.
//! There is no wildcard flag, so a new [`WorkerClass`] or job type needs its tag added to those commands by
//! hand. The matching worker type is caught at compile time by `enqueue_for_class`'s exhaustive
//! match; the command is not, and is an operational concern.

use std::sync::Arc;

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use sea_orm::DbErr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db_enum::db_enum;
use crate::error::YorishiroError;
use crate::models::entity_embeddings;
use crate::models::entity_entities;
use crate::workers::dispatch::EmbeddingSyncDispatcher;

db_enum! {
    /// Which class of worker process a queued job is meant for.
    ///
    /// Routes jobs by `BackgroundWorker::tags()`, which every provider honours, and also by a named queue per class (`queue()`), which only Redis honours: the SQL providers discard `queue: Option<String>` because their tables have no column for it.
    /// Redis filters tags client-side over the first 1000 entries of a queue, so without a queue per class a backlog of one class would hide every other class's jobs from its workers.
    /// `App::boot` adds these queue names to a Redis queue's `queues`, since Redis workers poll only the queues the configuration names.
    ///
    /// `tags()` takes no arguments and is called before a job's own `args` are seen (`loco-rs` 1.2.0's
    /// `perform_later_with_priority`), so one worker *type* carries one fixed tag set. A single type
    /// tagged with every class would put all three tags on every job, and a `--worker=worker-class:...`
    /// process would then dequeue every class's work rather than its own.
    ///
    /// Hence one type per class ([`EmbeddingSyncWorkerTenantPrivate`], [`EmbeddingSyncWorkerOfficial`],
    /// [`EmbeddingSyncWorkerShared`]), each fixed to a single tag, so the class picked at enqueue time
    /// is the tag that lands in the queue table.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum WorkerClass {
        /// Runs only on compute a single tenant registered for its own workspaces.
        TenantPrivate = "tenant_private",
        /// Runs only on compute this deployment operates itself.
        Official = "official",
        /// Runs on any worker process willing to take the job; the default for a deployment with no registered compute of its own.
        Shared = "shared",
    }
}

impl WorkerClass {
    /// The `tags()` value this class routes through.
    ///
    /// `worker-class:<variant>` rather than the bare variant name: a future tag dimension (region, priority band) added to the same job would otherwise collide on an unprefixed string with no way to tell which dimension it came from.
    pub fn tag(self) -> &'static str {
        match self {
            Self::TenantPrivate => "worker-class:tenant-private",
            Self::Official => "worker-class:official",
            Self::Shared => "worker-class:shared",
        }
    }

    /// The named queue this class routes through on a provider that honours one (Redis).
    ///
    /// Decided here rather than at each worker, so a class and its queue cannot drift apart; it merely equals the tag today.
    pub(crate) fn queue(self) -> &'static str {
        self.tag()
    }
}

/// Resolves a workspace's own worker-class assignment, if it has one.
///
/// A trait, the same shape as [`crate::services::embedding::WorkspaceEmbeddingResolver`], letting a deployment pin a workspace's jobs to particular compute without touching the callers that enqueue them.
/// [`DefaultWorkerClassResolver`] keeps every workspace on `Shared`.
///
/// `conn` is `ctx.db` rather than the RLS-scoped pool: which compute a tenant pays for is deployment configuration, not tenant content.
///
/// Returns `Ok(None)` for a workspace with no assignment, leaving the fallback to the caller rather than deciding it here.
#[async_trait]
pub trait WorkerClassResolver: Send + Sync {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError>;
}

/// This crate's own rule: no workspace has a worker-class assignment, so every job stays `Shared`.
pub(crate) struct DefaultWorkerClassResolver;

#[async_trait]
impl WorkerClassResolver for DefaultWorkerClassResolver {
    async fn resolve(
        &self,
        _conn: &sea_orm::DatabaseConnection,
        _workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError> {
        Ok(None)
    }
}

/// The resolver a deployment gets when it does not choose one.
#[must_use]
pub(crate) fn default_worker_class_resolver() -> Arc<dyn WorkerClassResolver> {
    Arc::new(DefaultWorkerClassResolver)
}

/// Ids only, not the `EntityRecord` or the provider's model name: everything is re-read inside `perform`, so an update racing ahead of a still-queued job is picked up as the entity's current state rather than overwritten with what was true at enqueue time.
/// A deleted entity is not found on that re-read, and the job is a no-op.
#[derive(Clone, Serialize, Deserialize)]
pub struct EmbeddingSyncArgs {
    #[serde(default)]
    pub lifecycle_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub entity_id: Uuid,
    pub worker_class: WorkerClass,
}

impl EmbeddingSyncArgs {
    /// The name this job is recorded under in the queue lifecycle.
    pub(crate) const JOB_NAME: &'static str = "embedding_sync";
}

#[derive(Debug)]
enum EmbeddingSyncOutcome {
    Persisted,
    Noop(&'static str),
    Superseded,
}

/// Keeps embedding-specific terminal outcomes in one place.
fn classify_embedding_error(error: YorishiroError) -> Result<EmbeddingSyncOutcome, YorishiroError> {
    match error {
        YorishiroError::Conflict { .. } => Ok(EmbeddingSyncOutcome::Superseded),
        error => Err(error),
    }
}

/// The provider a worker embeds `workspace_id`'s text with: the workspace's own, else the deployment default this worker process installed.
///
/// `Ok(None)` means neither exists, which is what a process that never installed a default provider sees.
///
/// # Errors
/// Returns `Internal` when the resolver seam is missing, or the resolver's own error.
pub(crate) async fn resolve_worker_provider(
    ctx: &AppContext,
    workspace_id: Uuid,
) -> Result<Option<Arc<dyn crate::services::embedding::EmbeddingProvider>>, YorishiroError> {
    let resolver = ctx
        .shared_store
        .get::<Arc<dyn crate::services::embedding::WorkspaceEmbeddingResolver>>()
        .ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!("WorkspaceEmbeddingResolver missing"))
        })?;
    Ok(match resolver.resolve(&ctx.db, workspace_id).await? {
        Some(provider) => Some(provider),
        None => ctx
            .shared_store
            .get::<Arc<dyn crate::services::embedding::EmbeddingProvider>>(),
    })
}

/// Runs one embedding attempt. It never re-enqueues itself.
async fn perform_embedding_sync(
    ctx: &AppContext,
    args: &EmbeddingSyncArgs,
) -> Result<EmbeddingSyncOutcome, YorishiroError> {
    let provider = resolve_worker_provider(ctx, args.workspace_id).await?;
    let Some(provider) = provider else {
        return Ok(EmbeddingSyncOutcome::Noop(
            "no embedding provider configured",
        ));
    };
    if provider.availability()
        == crate::services::embedding::EmbeddingProviderAvailability::Disabled
    {
        return Ok(EmbeddingSyncOutcome::Noop(
            "embedding provider explicitly disabled",
        ));
    }

    // Uses `ctx.db` (the identity pool) rather than `TenantDb::begin_for_workspace()`:
    // `entity_entities`, `entity_embeddings_*`, and `schema_schemas` have no row-level
    // security policies; every query scopes by `workspace_id` in its own WHERE clause,
    // which enforces the boundary regardless of the connection pool used. Switching to
    // `TenantDb` here would require resolving `tenant_id` from `workspace_id` first and
    // adds transaction-scoping overhead for zero additional isolation.
    let record = match entity_entities::get_with_token(&ctx.db, args.workspace_id, args.entity_id)
        .await
    {
        Ok(record) => record,
        Err(DbErr::RecordNotFound(_)) => {
            tracing::debug!(entity_id = %args.entity_id, "embedding sync worker: entity no longer exists, skipping");
            return Ok(EmbeddingSyncOutcome::Noop("entity deleted after enqueue"));
        }
        Err(err) => {
            tracing::warn!(entity_id = %args.entity_id, error = %err, "embedding sync worker: failed to re-read entity");
            return Err(YorishiroError::Internal(err.into()));
        }
    };

    let write = match entity_embeddings::sync_embedding_for_snapshot(
        &ctx.db,
        args.workspace_id,
        &record,
        provider.as_ref(),
    )
    .await
    {
        Ok(write) => write,
        Err(error) => return classify_embedding_error(error),
    };
    Ok(match write {
        entity_embeddings::EmbeddingWriteOutcome::Persisted => EmbeddingSyncOutcome::Persisted,
        entity_embeddings::EmbeddingWriteOutcome::Noop => {
            EmbeddingSyncOutcome::Noop("entity has no x-embed content")
        }
    })
}

async fn perform_embedding_with_lifecycle(
    ctx: &AppContext,
    args: &EmbeddingSyncArgs,
) -> loco_rs::Result<()> {
    let Some(id) = args.lifecycle_id else {
        return perform_embedding_sync(ctx, args)
            .await
            .map(|_| ())
            .map_err(Into::into);
    };
    let attempt = match crate::models::queue_job_lifecycles::Entity::start(&ctx.db, id)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?
    {
        crate::models::queue_job_lifecycles::Admission::Started { attempt }
        | crate::models::queue_job_lifecycles::Admission::Recovered { attempt } => attempt,
        crate::models::queue_job_lifecycles::Admission::Duplicate { .. }
        | crate::models::queue_job_lifecycles::Admission::Terminal => return Ok(()),
        crate::models::queue_job_lifecycles::Admission::Saturated { attempt } => {
            // Single-attempt policy: there is no requeue, so a capacity denial is terminal.
            // Leaving the row unfinished would orphan it as `queued` while Loco fails the job.
            const DIAGNOSTIC: &str = "embedding worker capacity saturated; not retried";
            let fenced = match attempt {
                Some(attempt) => {
                    crate::models::queue_job_lifecycles::Entity::finish(
                        &ctx.db,
                        id,
                        Some(attempt),
                        crate::models::queue_job_lifecycles::LifecycleStatus::Failed,
                        Some(DIAGNOSTIC),
                    )
                    .await
                }
                None => {
                    crate::models::queue_job_lifecycles::Entity::fail_unadmitted(
                        &ctx.db, id, DIAGNOSTIC,
                    )
                    .await
                }
            };
            return match fenced {
                Ok(()) => Err(loco_rs::Error::Message(DIAGNOSTIC.into())),
                // Another delivery claimed the row first and owns its lifecycle.
                Err(DbErr::RecordNotFound(_)) => Ok(()),
                Err(error) => Err(loco_rs::Error::Message(error.to_string())),
            };
        }
    };
    let result = perform_embedding_sync(ctx, args).await;
    let (status, diagnostic, error, superseded) = match result {
        Ok(EmbeddingSyncOutcome::Persisted) => (
            crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
            None,
            None,
            false,
        ),
        Ok(EmbeddingSyncOutcome::Noop(diagnostic)) => (
            crate::models::queue_job_lifecycles::LifecycleStatus::Completed,
            Some(diagnostic),
            None,
            false,
        ),
        Ok(EmbeddingSyncOutcome::Superseded) => (
            crate::models::queue_job_lifecycles::LifecycleStatus::Cancelled,
            Some("superseded by a newer entity update"),
            None,
            true,
        ),
        Err(error) => (
            crate::models::queue_job_lifecycles::LifecycleStatus::Failed,
            None,
            Some(error),
            false,
        ),
    };
    let error_diagnostic = error.as_ref().map(ToString::to_string);
    let diagnostic = diagnostic.or(error_diagnostic.as_deref());
    crate::models::queue_job_lifecycles::Entity::finish(
        &ctx.db,
        id,
        Some(attempt),
        status,
        diagnostic,
    )
    .await
    .map_err(|finish_error| loco_rs::Error::Message(finish_error.to_string()))?;
    if superseded {
        return Ok(());
    }
    error.map_or(Ok(()), |error| Err(error.into()))
}

/// Declares one `WorkerClass`'s worker type: a thin struct giving `tags()` a fixed single tag, so that class's jobs are visible only to a worker process asking for it.
macro_rules! embedding_sync_worker_for_class {
    ($worker_ty:ident, $class:expr) => {
        #[doc = concat!("`EmbeddingSyncWorker` restricted to `", stringify!($class), "` jobs.")]
        pub struct $worker_ty {
            ctx: AppContext,
        }

        #[async_trait]
        impl BackgroundWorker<EmbeddingSyncArgs> for $worker_ty {
            fn build(ctx: &AppContext) -> Self {
                Self { ctx: ctx.clone() }
            }

            fn tags() -> Vec<String> {
                vec![$class.tag().to_string()]
            }

            fn queue() -> Option<String> {
                Some($class.queue().to_string())
            }

            async fn perform(&self, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
                perform_embedding_with_lifecycle(&self.ctx, &args).await
            }
        }
    };
}

embedding_sync_worker_for_class!(EmbeddingSyncWorkerTenantPrivate, WorkerClass::TenantPrivate);
embedding_sync_worker_for_class!(EmbeddingSyncWorkerOfficial, WorkerClass::Official);
embedding_sync_worker_for_class!(EmbeddingSyncWorkerShared, WorkerClass::Shared);

/// Enqueues `args` on the worker type matching `args.worker_class`, so the queued tag is the one the caller resolved.
///
/// Exhaustively matched, with no `_` arm: a fourth `WorkerClass` without its worker type fails to compile rather than falling through to the wrong queue.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn enqueue_for_class(ctx: &AppContext, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
    let dispatcher = ctx
        .shared_store
        .get::<Arc<dyn EmbeddingSyncDispatcher>>()
        .ok_or_else(|| loco_rs::Error::Message("EmbeddingSyncDispatcher missing".into()))?;
    enqueue_for_class_with_dispatcher(ctx, args, dispatcher.as_ref()).await
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn enqueue_for_class_with_dispatcher(
    ctx: &AppContext,
    args: EmbeddingSyncArgs,
    dispatcher: &dyn EmbeddingSyncDispatcher,
) -> loco_rs::Result<()> {
    dispatcher.dispatch(ctx, args).await.map(|_job_id| ())
}

/// Enqueues embedding sync after the caller's own transaction has committed: generating a vector is an HTTP round trip to the embedding provider (up to 30s), and this must never add that latency to the entity write it follows, nor hold a DB connection open for it.
/// The application uses the persistent queue mode, so `perform_later_with_priority` persists the job and returns before the embedding provider round trip begins.
/// A process restart or forced kill leaves the job in the configured provider for the next worker run.
/// A failure to enqueue at all (queue provider unreachable) is only logged: the entity write already succeeded and embedding is an auxiliary feature, so no failure here should surface to the caller.
///
/// This lives here rather than beside one transport's handlers because both of them need it: every entity write that does not call this leaves `entity_entities.embedding` NULL forever, and such an entity is reachable only through the `pg_trgm` fuzzy fallback, so the symptom is search quietly returning worse results rather than any error.
pub(crate) async fn enqueue_after_write(ctx: &AppContext, workspace_id: Uuid, entity_id: Uuid) {
    let dispatcher = match ctx.shared_store.get::<Arc<dyn EmbeddingSyncDispatcher>>() {
        Some(dispatcher) => dispatcher,
        None => {
            tracing::warn!(entity_id = %entity_id, "EmbeddingSyncDispatcher missing");
            return;
        }
    };
    if let Err(err) =
        enqueue_after_write_with_dispatcher(ctx, workspace_id, entity_id, dispatcher.as_ref()).await
    {
        tracing::warn!(entity_id = %entity_id, error = %err, "failed to enqueue embedding sync");
    }
}

pub(crate) async fn enqueue_after_write_with_dispatcher(
    ctx: &AppContext,
    workspace_id: Uuid,
    entity_id: Uuid,
    dispatcher: &dyn EmbeddingSyncDispatcher,
) -> loco_rs::Result<()> {
    let worker_class = match crate::controllers::extractors::resolve_worker_class(ctx, workspace_id)
        .await
    {
        Ok(worker_class) => worker_class,
        Err(err) => {
            tracing::warn!(entity_id = %entity_id, error = %err.0, "failed to resolve worker class, defaulting to shared");
            WorkerClass::Shared
        }
    };
    let args = EmbeddingSyncArgs {
        lifecycle_id: None,
        workspace_id,
        entity_id,
        worker_class,
    };
    enqueue_for_class_with_dispatcher(ctx, args, dispatcher).await
}
