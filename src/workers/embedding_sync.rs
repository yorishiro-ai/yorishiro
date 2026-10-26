//! Generates and stores an entity's embedding vector via Loco's persistent background queue.
//!
//! A queue provider rather than a bare `tokio::spawn`: a spawned task loses every in-flight sync on a process restart, a forced kill, or a provider outage past its own retry budget, leaving the entity's `embedding` column permanently `NULL` with nothing to retry it (`tasks::resync_embeddings` is the operational recovery command for rows in that state).
//! The configured Loco queue provider persists the job, so a re-deployed or restarted process resumes it instead of losing it.
//!
//! Every job this module enqueues carries exactly one tag, the `worker-target:<name>` of the target it is for (see [`crate::workers::target`]).
//! An empty tag list makes loco's dequeue query `AND (tags IS NULL)` (confirmed against the pinned `loco-rs` 1.2.0 `bgworker/pg.rs`, and the matching logic in `sqlt.rs`/`redis.rs`), so a worker that names no target would take none of them.
//! `App::boot` therefore refuses a bare `--worker` and any tag that names no target.

use std::sync::Arc;

use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::async_trait;
use sea_orm::DbErr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db_enum::db_enum;
use crate::error::YorishiroError;
use crate::models::entity_embeddings;
use crate::models::entity_entities;

db_enum! {
    /// What kind of compute a queued job is meant for.
    ///
    /// The class is recorded on the job's lifecycle row and decides its priority band and concurrency limit.
    /// It does not choose the worker: that is the job's [`crate::workers::target::WorkerTarget`], which a `TenantPrivate` job derives from the tenant that owns its workspace.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum WorkerClass {
        /// Runs only on compute a single tenant registered for its own workspaces.
        TenantPrivate = "tenant_private",
        /// Runs only on compute this deployment operates itself.
        Official = "official",
        /// Runs on any worker process willing to take the job; the default for a deployment with no registered compute of its own.
        Shared = "shared",
    }
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

/// Runs one embedding attempt. It never re-enqueues itself.
async fn perform_embedding_sync(
    ctx: &AppContext,
    args: &EmbeddingSyncArgs,
) -> Result<EmbeddingSyncOutcome, YorishiroError> {
    let provider =
        crate::models::workspace_workspaces::embedding_provider(ctx, args.workspace_id).await?;
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
        .get::<Arc<tokio::sync::Semaphore>>()
        .ok_or_else(|| loco_rs::Error::Message("embedding concurrency missing".into()))?;
    let _permit = concurrency
        .acquire_owned()
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
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
/// It declares no tags and no queue: the durable dispatcher records a job's resolved target, and a worker process chooses which targets to serve when it starts.
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

/// Enqueues embedding work through the durable outbox.
///
/// # Errors
/// Returns an error if the durable enqueue fails.
pub async fn enqueue_for_class(ctx: &AppContext, args: EmbeddingSyncArgs) -> loco_rs::Result<()> {
    crate::workers::dispatch::dispatch_embedding(ctx, args, None)
        .await
        .map(|_| ())
}

pub(crate) async fn enqueue_after_write(
    ctx: &AppContext,
    workspace_id: Uuid,
    entity_id: Uuid,
    txn: &sea_orm::DatabaseTransaction,
) -> Result<(), YorishiroError> {
    let worker_class = crate::controllers::extractors::resolve_worker_class(ctx, workspace_id)
        .await
        .map_err(|error| error.0)?;
    let args = EmbeddingSyncArgs {
        lifecycle_id: None,
        workspace_id,
        entity_id,
        worker_class,
    };
    crate::workers::dispatch::dispatch_embedding(ctx, args, Some(txn))
        .await
        .map(|_| ())
        .map_err(|error| YorishiroError::Internal(anyhow::anyhow!(error.to_string())))
}
