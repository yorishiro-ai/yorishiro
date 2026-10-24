//! Snapshot restoration runs on the configured Loco queue, not the request task.

use loco_rs::{app::AppContext, bgworker::BackgroundWorker, prelude::async_trait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::{api_key_audit_log, api_keys::AuthContext, entity_entities};
use crate::workers::{
    dispatch::{JobSpec, dispatch_job},
    embedding_sync::WorkerClass,
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct UndoArgs {
    lifecycle_id: Option<Uuid>,
    tenant_id: Uuid,
    workspace_id: Uuid,
    api_key_id: Uuid,
    user_id: Option<Uuid>,
    snapshot_job_id: Uuid,
    worker_class: WorkerClass,
}

pub(crate) struct UndoWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<UndoArgs> for UndoWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: UndoArgs) -> loco_rs::Result<()> {
        super::lifecycle::perform_with_terminal_lifecycle::<Self, _, _, _>(
            &self.ctx,
            args.lifecycle_id,
            args.worker_class,
            args.workspace_id,
            &args,
            || async {
                self.restore(&args)
                    .await
                    .map_err(|error| loco_rs::Error::Message(error.to_string()))
            },
        )
        .await
    }
}

impl UndoWorker {
    async fn restore(&self, args: &UndoArgs) -> Result<(), YorishiroError> {
        let id = args
            .lifecycle_id
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("undo lifecycle missing")))?;
        let txn =
            crate::db::begin_workspace_write(&self.ctx, args.tenant_id, args.workspace_id).await?;
        // Concurrent redeliveries serialize with the completion marker and restoration.
        crate::db::lock_for_update(&txn, &format!("undo:{id}"))
            .await
            .internal()?;
        if api_key_audit_log::job_completed(
            &txn,
            args.workspace_id,
            id,
            api_key_audit_log::AuditAction::UndoMigrationJob,
        )
        .await?
        {
            txn.commit().await.internal()?;
            return Ok(());
        }
        let report =
            entity_entities::undo_job(&txn, args.workspace_id, args.snapshot_job_id).await?;
        api_key_audit_log::record(
            &txn,
            api_key_audit_log::AuditActor {
                tenant_id: args.tenant_id,
                workspace_id: args.workspace_id,
                api_key_id: args.api_key_id,
                user_id: args.user_id,
            },
            api_key_audit_log::AuditAction::UndoMigrationJob,
            serde_json::json!({"job_id": id, "snapshot_job_id": args.snapshot_job_id,
                "restored": report.restored, "missing": report.missing}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }
}

pub(crate) async fn enqueue(
    ctx: &AppContext,
    txn: &sea_orm::DatabaseTransaction,
    auth: &AuthContext,
    snapshot_job_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    entity_entities::require_job_snapshots(txn, auth.workspace_id, snapshot_job_id).await?;
    let worker_class = crate::controllers::extractors::resolve_worker_class(ctx, auth.workspace_id)
        .await
        .map_err(|error| error.0)?;
    let args = UndoArgs {
        lifecycle_id: None,
        tenant_id: auth.tenant_id,
        workspace_id: auth.workspace_id,
        api_key_id: auth.api_key_id,
        user_id: auth.user_id,
        snapshot_job_id,
        worker_class,
    };
    let id = dispatch_job(
        ctx,
        JobSpec {
            job_name: "undo",
            workspace_id: auth.workspace_id,
            class: worker_class,
            worker_name: UndoWorker::class_name(),
            dispatch_payload: serde_json::to_value(args).internal()?,
            use_queue_concurrency: true,
        },
        Some(txn),
    )
    .await
    .internal()?;
    id.parse().internal()
}
