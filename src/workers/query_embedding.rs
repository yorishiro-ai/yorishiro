//! Embeds a search query on a worker, because the API and MCP server holds no embedding model.
//!
//! The server opens a `pending` row in `query_embedding_requests`, enqueues a [`QueryEmbeddingWorker`] job that names it, and polls the row for a bounded time.
//! The worker reads the text, resolves the workspace's provider, embeds it as a query, checks the width against the workspace's tables, and completes the row.
//! A failure is recorded on the row and also returned to Loco, so the provider job shows as failed.
//!
//! The job is not admitted through the lifecycle table: a search is waiting for it, so it must neither queue behind a class's concurrency limit nor be retried after the request has expired.
//! It has its own tag and queue, so a deployment can dedicate workers to queries, and every worker started with all tags handles both documents and queries.
//! A job whose row is gone or no longer pending is a late delivery and does nothing.

use std::time::Duration;

use async_trait::async_trait;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use sea_orm::{DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use uuid::Uuid;

use crate::data::settings::{QueryEmbedding, Settings};
use crate::db::AppContextBackend;
use crate::error::YorishiroError;
use crate::models::query_embedding_requests::{Entity as Requests, QueryOutcome};
use crate::models::search;
use crate::services::embedding::EmbedKind;
use crate::workers::embedding_sync::resolve_worker_provider;

/// The tag and queue query jobs travel on.
const TAG: &str = "query-embedding";

/// Above every document class band in `workers::queue::priority`, so a waiting search is dequeued before document backlog.
const PRIORITY: i32 = 1000;

/// How long a client is told to wait before retrying a search no worker answered.
const RETRY_AFTER: Duration = Duration::from_secs(1);

/// What a query job carries: which request to answer and whose it is.
/// The text stays in the row, so the job payload holds no user content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryEmbeddingArgs {
    pub request_id: Uuid,
    pub workspace_id: Uuid,
}

/// The worker type for [`QueryEmbeddingArgs`].
pub struct QueryEmbeddingWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<QueryEmbeddingArgs> for QueryEmbeddingWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    fn tags() -> Vec<String> {
        vec![TAG.to_owned()]
    }

    fn queue() -> Option<String> {
        Some(TAG.to_owned())
    }

    async fn perform(&self, args: QueryEmbeddingArgs) -> loco_rs::Result<()> {
        // The message, not the HTTP-shaped conversion: Loco stores this text as the failed job's diagnostic, so an operator reads the cause in the queue.
        answer(&self.ctx, &args)
            .await
            .map_err(|error| loco_rs::Error::Message(error.to_string()))
    }
}

/// Answers one request: stores the vector, or records why it could not.
async fn answer(ctx: &AppContext, args: &QueryEmbeddingArgs) -> Result<(), YorishiroError> {
    let workspace_id = args.workspace_id;
    if let Err(error) = Requests::purge_expired(&ctx.db).await {
        tracing::warn!(error = %error, "query embedding: could not purge expired requests");
    }
    let Some(request) = Requests::find_pending(&ctx.db, workspace_id, args.request_id)
        .await
        .map_err(|error| YorishiroError::Internal(error.into()))?
    else {
        tracing::debug!(request_id = %args.request_id, "query embedding: request is gone or no longer pending");
        return Ok(());
    };

    match embed(ctx, workspace_id, &request.query_text).await {
        Ok((vector, model)) => {
            let stored = Requests::complete(&ctx.db, workspace_id, request.id, &vector, &model)
                .await
                .map_err(|error| YorishiroError::Internal(error.into()))?;
            if !stored {
                tracing::info!(request_id = %request.id, "query embedding: the search stopped waiting, result discarded");
            }
            Ok(())
        }
        Err(error) => {
            if let Err(record) =
                Requests::fail(&ctx.db, workspace_id, request.id, &error.to_string()).await
            {
                tracing::warn!(request_id = %request.id, error = %record, "query embedding: could not record the failure");
            }
            Err(error)
        }
    }
}

async fn embed(
    ctx: &AppContext,
    workspace_id: Uuid,
    query_text: &str,
) -> Result<(Vec<f32>, String), YorishiroError> {
    let provider = resolve_worker_provider(ctx, workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::BackendUnavailable {
            message: "no embedding provider is configured on the worker".into(),
        })?;
    let concurrency = ctx
        .shared_store
        .get::<crate::services::embedding::concurrency::EmbeddingConcurrency>()
        .ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!("embedding concurrency missing"))
        })?;
    let _permit = concurrency
        .acquire()
        .await
        .map_err(|error| YorishiroError::Internal(error.into()))?;
    let vector = provider.embed_as(EmbedKind::Query, query_text).await?;
    search::check_query_width(&ctx.db, workspace_id, vector.len()).await?;
    Ok((vector, provider.model_name()))
}

/// Embeds `query_text` for a search by asking a worker, and waits a bounded time for the answer.
///
/// `tenant_id` and `workspace_id` come from the verified API key.
/// On PostgreSQL every read and write of the request row runs on the tenant pool inside that workspace's scope, so row level security applies; SQLite is single-tenant.
/// There is no fallback: when no worker answers in time the search fails, because a lexical result would silently differ from the semantic one the caller asked for.
///
/// # Errors
/// - `ProviderBusy` (503 with a retry hint) when no worker answered within `timeout_ms`.
/// - `BackendUnavailable` (503) when the job could not be queued or the worker recorded a failure.
/// - `Internal` when the request row cannot be read or holds a malformed result.
pub async fn embed_query(
    ctx: &AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
    query_text: &str,
) -> Result<Vec<f32>, YorishiroError> {
    let settings = ctx
        .shared_store
        .get::<Settings>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("application settings missing")))?
        .query_embedding;
    let scope = Scope {
        ctx,
        tenant_id,
        workspace_id,
    };

    let request_id = scope.open(query_text, settings.retention_seconds).await?;
    let job = QueryEmbeddingArgs {
        request_id,
        workspace_id,
    };
    if let Err(error) =
        QueryEmbeddingWorker::perform_later_with_priority(ctx, job, Some(PRIORITY)).await
    {
        // Nothing will answer this row, so remove it rather than leave it to expire.
        if matches!(scope.expire(request_id).await, Ok(true)) {
            let _ = scope.read(request_id).await;
        }
        return Err(YorishiroError::BackendUnavailable {
            message: format!("the query could not be queued for embedding: {error}"),
        });
    }

    wait(&scope, request_id, settings).await
}

/// Polls until the request finishes or `timeout_ms` has passed, then expires it.
///
/// Each iteration sleeps at most `poll_interval_ms` and never past the deadline, so the number of reads is bounded by `timeout_ms / poll_interval_ms + 1`.
async fn wait(
    scope: &Scope<'_>,
    request_id: Uuid,
    settings: QueryEmbedding,
) -> Result<Vec<f32>, YorishiroError> {
    let interval = Duration::from_millis(settings.poll_interval_ms);
    let deadline = Instant::now() + Duration::from_millis(settings.timeout_ms);
    loop {
        match scope.read(request_id).await? {
            Some(QueryOutcome::Pending) => {}
            finished => return finished_result(finished),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(interval.min(remaining)).await;
    }

    // Fence the timeout: a worker that completed the row between the last read and now wins, and its result is read instead of discarded.
    if scope.expire(request_id).await? {
        scope.read(request_id).await?;
        return Err(YorishiroError::ProviderBusy {
            message: format!(
                "no embedding worker answered the query within {} ms",
                settings.timeout_ms
            ),
            retry_after: RETRY_AFTER,
        });
    }
    finished_result(scope.read(request_id).await?)
}

fn finished_result(outcome: Option<QueryOutcome>) -> Result<Vec<f32>, YorishiroError> {
    match outcome {
        Some(QueryOutcome::Ready { vector, .. }) => Ok(vector),
        Some(QueryOutcome::Failed(message)) => Err(YorishiroError::BackendUnavailable {
            message: format!("the query could not be embedded: {message}"),
        }),
        Some(QueryOutcome::Expired | QueryOutcome::Pending) => Err(YorishiroError::ProviderBusy {
            message: "the query embedding request expired before a worker answered".into(),
            retry_after: RETRY_AFTER,
        }),
        None => Err(YorishiroError::BackendUnavailable {
            message: "the query embedding request was purged before its result was read".into(),
        }),
    }
}

/// One workspace's view of the request table: a short transaction per step, so no connection is held while waiting.
struct Scope<'a> {
    ctx: &'a AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
}

impl Scope<'_> {
    async fn begin(&self) -> Result<DatabaseTransaction, YorishiroError> {
        if self.ctx.is_sqlite() {
            return self
                .ctx
                .db
                .begin()
                .await
                .map_err(|error| YorishiroError::Internal(error.into()));
        }
        let db = crate::controllers::extractors::db_handle(self.ctx).map_err(|error| error.0)?;
        db.tenant
            .begin_for_workspace(self.tenant_id, self.workspace_id)
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))
    }

    async fn open(&self, query_text: &str, retention_seconds: u64) -> Result<Uuid, YorishiroError> {
        let txn = self.begin().await?;
        if let Err(error) = Requests::purge_expired(&txn).await {
            tracing::warn!(error = %error, "query embedding: could not purge expired requests");
        }
        let id = Requests::open(&txn, self.workspace_id, query_text, retention_seconds)
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        txn.commit()
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        Ok(id)
    }

    async fn read(&self, request_id: Uuid) -> Result<Option<QueryOutcome>, YorishiroError> {
        let txn = self.begin().await?;
        let outcome = Requests::take(&txn, self.workspace_id, request_id).await?;
        txn.commit()
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        Ok(outcome)
    }

    async fn expire(&self, request_id: Uuid) -> Result<bool, YorishiroError> {
        let txn = self.begin().await?;
        let expired = Requests::expire(&txn, self.workspace_id, request_id)
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        txn.commit()
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        Ok(expired)
    }
}
