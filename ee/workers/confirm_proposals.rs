//! Proposal confirmation runs on the configured Loco provider.

use loco_rs::{app::AppContext, bgworker::BackgroundWorker, prelude::async_trait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::{
    api_key_audit_log::{self, AuditAction, AuditActor},
    api_keys::AuthContext,
};
use crate::workers::{
    dispatch::{JobSpec, dispatch_job},
    embedding_sync::WorkerClass,
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ConfirmProposalsArgs {
    lifecycle_id: Option<Uuid>,
    tenant_id: Uuid,
    workspace_id: Uuid,
    api_key_id: Uuid,
    user_id: Option<Uuid>,
    inference_job_id: Uuid,
    worker_class: WorkerClass,
}

pub(crate) struct ConfirmProposalsWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<ConfirmProposalsArgs> for ConfirmProposalsWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: ConfirmProposalsArgs) -> loco_rs::Result<()> {
        crate::workers::lifecycle::perform_with_terminal_lifecycle::<Self, _, _, _>(
            &self.ctx,
            args.lifecycle_id,
            args.worker_class,
            args.workspace_id,
            &args,
            || async {
                self.confirm(&args)
                    .await
                    .map_err(|error| loco_rs::Error::Message(error.to_string()))
            },
        )
        .await
    }
}

impl ConfirmProposalsWorker {
    async fn confirm(&self, args: &ConfirmProposalsArgs) -> Result<(), YorishiroError> {
        let id = args.lifecycle_id.ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!("confirmation lifecycle missing"))
        })?;
        let txn =
            crate::db::begin_workspace_write(&self.ctx, args.tenant_id, args.workspace_id).await?;
        crate::db::lock_for_update(&txn, &format!("confirm-proposals:{id}"))
            .await
            .internal()?;
        if !api_key_audit_log::job_completed(
            &txn,
            args.workspace_id,
            id,
            AuditAction::BackgroundJobCompleted,
        )
        .await?
        {
            crate::models::access::require_job_actor(
                &txn,
                &self.ctx.db,
                &AuditActor {
                    tenant_id: args.tenant_id,
                    workspace_id: args.workspace_id,
                    api_key_id: args.api_key_id,
                    user_id: args.user_id,
                },
                crate::models::api_keys::ApiKeyScope::Schema,
            )
            .await?;
            let report = crate::edition::ee::models::inference_proposals::confirm(
                &txn,
                args.workspace_id,
                args.inference_job_id,
                args.user_id,
            )
            .await?;
            api_key_audit_log::record(&txn, AuditActor {
                tenant_id: args.tenant_id, workspace_id: args.workspace_id,
                api_key_id: args.api_key_id, user_id: args.user_id,
            }, AuditAction::BackgroundJobCompleted, serde_json::json!({"job_id": id, "inference_job_id": args.inference_job_id, "confirmed": report.confirmed, "stale": report.stale, "invalid": report.invalid})).await?;
        }
        txn.commit().await.internal()?;
        Ok(())
    }
}

pub(crate) async fn enqueue(
    ctx: &AppContext,
    txn: &sea_orm::DatabaseTransaction,
    auth: &AuthContext,
    inference_job_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    crate::edition::ee::models::inference_proposals::require_completed_job(
        txn,
        auth.workspace_id,
        inference_job_id,
    )
    .await?;
    let worker_class = crate::controllers::extractors::resolve_worker_class(ctx, auth.workspace_id)
        .await
        .map_err(|error| error.0)?;
    let args = ConfirmProposalsArgs {
        lifecycle_id: None,
        tenant_id: auth.tenant_id,
        workspace_id: auth.workspace_id,
        api_key_id: auth.api_key_id,
        user_id: auth.user_id,
        inference_job_id,
        worker_class,
    };
    let id = dispatch_job(
        ctx,
        JobSpec {
            job_name: "confirm_proposals",
            workspace_id: auth.workspace_id,
            class: worker_class,
            worker_name: ConfirmProposalsWorker::class_name(),
            dispatch_payload: serde_json::to_value(args).internal()?,
            use_queue_concurrency: true,
        },
        Some(txn),
    )
    .await
    .internal()?;
    id.parse().internal()
}
