//! Background worker for workspace reindex operations.
//!
//! This worker re-embeds all entities in a workspace with the current provider model,
//! then restamps the workspace's `embedding_model`/`embedding_dimensions`.
//!
//! The REST endpoint (`POST /api/migration-jobs/reindex`) enqueues this worker, which runs under the workspace advisory lock to serialize reindex operations per workspace.
//!
//! **Routing**: like `embedding_sync`, a reindex job carries one `worker-target:<name>` tag chosen when it is enqueued,
//! so `--worker=worker-target:tenant:<id>` picks up only that tenant's reindex jobs.

use crate::models::iam::session::WorkspaceSession;
use loco_rs::app::AppContext;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::prelude::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::YorishiroError;
use crate::services::embedding::EmbeddingProviderAvailability;
use crate::workers::embedding_sync::WorkerClass;

/// Arguments for a reindex worker job: workspace id and the resolved worker class tag.
///
/// `worker_class` determines which tag the job lands under in the queue, so that
/// tag-restricted worker processes dequeue only their own jobs.
#[derive(Clone, Serialize, Deserialize)]
pub struct ReindexArgs {
    #[serde(default)]
    pub lifecycle_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub worker_class: WorkerClass,
    #[serde(default)]
    pub startup: bool,
    #[serde(default)]
    pub actor: Option<ReindexActor>,
}

#[derive(Clone, Serialize, Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "identifier field names are part of the persisted reindex job format"
)]
pub struct ReindexActor {
    pub tenant_id: Uuid,
    pub access_key_id: Uuid,
    pub user_id: Option<Uuid>,
}

impl ReindexArgs {
    /// The name this job is recorded under in the queue lifecycle.
    pub(crate) const JOB_NAME: &'static str = "reindex";
    pub(crate) const STARTUP_JOB_NAME: &'static str = "startup_reindex";

    pub(crate) const fn job_name(&self) -> &'static str {
        if self.startup {
            Self::STARTUP_JOB_NAME
        } else {
            Self::JOB_NAME
        }
    }
}

async fn perform_reindex(ctx: &AppContext, args: &ReindexArgs) -> loco_rs::Result<()> {
    let actor = args
        .actor
        .as_ref()
        .map(|actor| crate::models::access_keys::AuthContext {
            tenant_id: actor.tenant_id,
            access_key_id: actor.access_key_id,
            user_id: actor.user_id,
            workspace_id: Some(args.workspace_id),
        });
    if let Some(actor) = &actor {
        let txn = crate::db::begin_write(&ctx.db).await?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            crate::models::iam::policy::Action::ReindexWorkspace,
            crate::models::iam::policy::Resource::Workspace {
                tenant_id: actor.tenant_id,
                workspace_id: args.workspace_id,
            },
        )
        .await
        .map_err(loco_rs::Error::wrap)?;
        txn.commit().await?;
    }
    let concurrency = ctx
        .shared_store
        .get::<std::sync::Arc<tokio::sync::Semaphore>>()
        .ok_or_else(|| loco_rs::Error::Message("embedding concurrency missing".into()))?;
    let _permit = concurrency
        .acquire_owned()
        .await
        .map_err(loco_rs::Error::wrap)?;
    crate::models::entity_embeddings::with_reindex_lock(ctx, args.workspace_id, async || {
        // A provider change or entity write can invalidate a reindex even while it holds the reindex lock.
        for _ in 0..2 {
            let result = reindex_current_assignment(ctx, args, actor.as_ref()).await;
            if !matches!(result, Err(YorishiroError::Conflict { .. })) {
                return result;
            }
        }
        reindex_current_assignment(ctx, args, actor.as_ref()).await
    })
    .await
    .map_err(loco_rs::Error::wrap)
}

async fn reindex_current_assignment(
    ctx: &AppContext,
    args: &ReindexArgs,
    actor: Option<&crate::models::access_keys::AuthContext>,
) -> Result<(), YorishiroError> {
    // Resolve after taking the reindex lock so a preceding reindex cannot invalidate this snapshot.
    let provider = crate::models::workspace_workspaces::embedding_provider(ctx, args.workspace_id)
        .await?
        .filter(|provider| provider.availability() != EmbeddingProviderAvailability::Disabled)
        .ok_or_else(|| YorishiroError::BackendUnavailable {
            message: "no embedding provider is configured".into(),
        })?;
    let outcome = crate::models::entity_embeddings::reindex_workspace_as(
        &ctx.db,
        args.workspace_id,
        provider.as_ref(),
        actor,
        args.startup,
    )
    .await?;
    if !outcome.failures.is_empty() {
        for failure in &outcome.failures {
            tracing::warn!(
                workspace_id = %args.workspace_id,
                entity_id = %failure.entity_id,
                lifecycle_id = ?args.lifecycle_id,
                error_code = failure.error.code().map_or("internal", |code| code.as_str()),
                "reindex could not prepare an entity embedding"
            );
        }
        record_failed_entities(ctx, args, &outcome).await;
        tracing::warn!(
            workspace_id = %args.workspace_id,
            total = outcome.total,
            failed = outcome.failures.len(),
            "reindex could not prepare all workspace embeddings"
        );
        return Err(YorishiroError::BackendUnavailable {
            message: format!(
                "reindex could not prepare embeddings for {} of {} entities",
                outcome.failures.len(),
                outcome.total
            ),
        });
    }

    Ok(())
}

/// The worker that reindexes one workspace.
///
/// It declares no tags and no queue: the durable dispatcher records the resolved target in the outbox.
pub struct ReindexWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<ReindexArgs> for ReindexWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: ReindexArgs) -> loco_rs::Result<()> {
        crate::workers::lifecycle::perform_with_terminal_lifecycle::<Self, _, _, _>(
            &self.ctx,
            args.lifecycle_id,
            args.worker_class,
            args.workspace_id,
            &args,
            || perform_reindex(&self.ctx, &args),
        )
        .await
    }
}

/// Enqueues a reindex job through the durable outbox.
///
/// # Errors
/// Returns an error if the durable enqueue fails.
pub async fn enqueue_for_class(ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<()> {
    crate::workers::dispatch::dispatch_reindex(ctx, args, None)
        .await
        .map(|_| ())
}

/// Enqueues reindex in the caller transaction when supplied.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn enqueue_reindex(
    ctx: &AppContext,
    workspace_id: Uuid,
    txn: Option<&sea_orm::DatabaseTransaction>,
) -> loco_rs::Result<String> {
    if ctx.queue_provider.is_none() {
        return Err(loco_rs::Error::Message(
            "no queue provider configured".into(),
        ));
    }

    let worker_class = crate::workers::target::resolve_worker_class(ctx, workspace_id)
        .await
        .map_err(loco_rs::Error::from)?;
    crate::workers::dispatch::dispatch_reindex(
        ctx,
        ReindexArgs {
            lifecycle_id: None,
            workspace_id,
            worker_class,
            startup: false,
            actor: None,
        },
        txn,
    )
    .await
}

/// Opens the caller's migration session, enqueues a reindex of its workspace and commits.
///
/// # Errors
/// Returns the session, authorization or enqueue error.
pub(crate) async fn enqueue_requested_for_actor(
    ctx: &AppContext,
    actor: crate::models::access_keys::AuthContext,
) -> Result<String, YorishiroError> {
    WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::ReindexWorkspace,
        ),
        None,
        async |session| enqueue_requested(ctx, session.txn(), &session.ctx).await,
    )
    .await
}

pub(crate) async fn enqueue_requested(
    ctx: &AppContext,
    txn: &sea_orm::DatabaseTransaction,
    actor: &crate::models::access_keys::AuthContext,
) -> Result<String, YorishiroError> {
    use crate::error::ResultExt;
    if ctx.queue_provider.is_none() {
        return Err(YorishiroError::BackendUnavailable {
            message: "reindex requires a queue provider (configure queue: in the server config)"
                .into(),
        });
    }
    let authorization = crate::db::begin_authorization(&ctx.db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        authorization.as_ref().unwrap_or(txn),
        actor,
        crate::models::iam::policy::Action::ReindexWorkspace,
        crate::models::iam::policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id: actor.require_workspace()?,
        },
    )
    .await?;
    let worker_class =
        crate::workers::target::resolve_worker_class(ctx, actor.require_workspace()?).await?;
    let id = crate::workers::dispatch::dispatch_reindex(
        ctx,
        ReindexArgs {
            lifecycle_id: None,
            workspace_id: actor.require_workspace()?,
            worker_class,
            startup: false,
            actor: Some(ReindexActor {
                tenant_id: actor.tenant_id,
                access_key_id: actor.access_key_id,
                user_id: actor.user_id,
            }),
        },
        Some(txn),
    )
    .await
    .internal()?;
    if let Some(authorization) = authorization {
        authorization.commit().await.internal()?;
    }
    Ok(id)
}

/// Leaves one trail row naming every entity the reindex could not prepare, with each one's error code.
async fn record_failed_entities(
    ctx: &AppContext,
    args: &ReindexArgs,
    outcome: &crate::models::entity_embeddings::ReindexOutcome,
) {
    let Some(lifecycle_id) = args.lifecycle_id else {
        return;
    };
    let request_id = crate::models::queue_job_lifecycles::Entity::request_id(&ctx.db, lifecycle_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "req-id-none".to_owned());
    let failed: Vec<_> = outcome
        .failures
        .iter()
        .map(|failure| {
            serde_json::json!({
                "entity_id": failure.entity_id,
                "error_code": failure.error.code().map_or("internal", |code| code.as_str()),
            })
        })
        .collect();
    let row = crate::models::api_events::Call::job(
        request_id,
        lifecycle_id,
        format!("{}.entities", args.job_name()),
        Some(args.workspace_id),
        Some("failed"),
        0,
    )
    .with_resources(serde_json::Value::Array(failed));
    if let Err(error) = row.append(&ctx.db).await {
        tracing::error!(%lifecycle_id, %error, "the failed entities could not be recorded in the trail");
    }
}
