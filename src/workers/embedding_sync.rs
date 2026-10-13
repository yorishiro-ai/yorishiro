//! Generates and stores an entity's embedding vector via Loco's persistent background queue.
//!
//! A queue provider rather than a bare `tokio::spawn`: a spawned task loses every in-flight sync on a process restart, a forced kill, or a provider outage past its own retry budget, leaving the entity's `embedding` column permanently `NULL` with nothing to retry it (`tasks::resync_embeddings` is the operational recovery command for rows in that state).
//! The configured Loco queue provider persists the job, so a re-deployed or restarted process resumes it instead of losing it.
//!
//! Every job this module enqueues carries exactly one tag, the `worker-target:<name>` of the target it is for (see [`crate::workers::target`]).
//! An empty tag list makes loco's dequeue query `AND (tags IS NULL)` (confirmed against the pinned `loco-rs` 1.2.0 `bgworker/pg.rs`, and the matching logic in `sqlt.rs`/`redis.rs`), so a worker that names no target would take none of them.
//! `App::boot` therefore reads a bare `--worker` as the shared target, and refuses any tag that names no target.

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
    /// What kind of compute a queued job is meant for.
    ///
    /// The class is recorded on the job's lifecycle row and decides its priority band and concurrency limit.
    /// It does not choose the worker: that is the job's [`crate::workers::target::WorkerTarget`], which a `TenantPrivate` job derives from the tenant that owns its workspace.
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
    let concurrency = ctx
        .shared_store
        .get::<crate::services::embedding::concurrency::EmbeddingConcurrency>()
        .ok_or_else(|| loco_rs::Error::Message("embedding concurrency missing".into()))?;
    let _permit = concurrency
        .acquire()
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let Some(id) = args.lifecycle_id else {
        return perform_embedding_sync(ctx, args)
            .await
            .map(|_| ())
            .map_err(Into::into);
    };
    let attempt =
        match crate::models::queue_job_lifecycles::Entity::start_ignoring_capacity(&ctx.db, id)
            .await
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?
        {
            crate::models::queue_job_lifecycles::Admission::Started { attempt }
            | crate::models::queue_job_lifecycles::Admission::Recovered { attempt } => attempt,
            crate::models::queue_job_lifecycles::Admission::Duplicate { .. }
            | crate::models::queue_job_lifecycles::Admission::Terminal => return Ok(()),
            crate::models::queue_job_lifecycles::Admission::Saturated { .. } => {
                unreachable!("capacity is non-authoritative for embedding workers")
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

/// The worker that embeds one entity.
///
/// It declares no tags and no queue: a job's target is chosen when the job is enqueued ([`crate::workers::target::enqueue`]), and a worker process chooses which targets to serve when it starts.
pub struct EmbeddingSyncWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<EmbeddingSyncArgs> for EmbeddingSyncWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
        perform_embedding_with_lifecycle(&self.ctx, &args).await
    }
}

/// Enqueues `args` through the installed dispatcher, which picks the target from `args.worker_class` and the workspace.
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
