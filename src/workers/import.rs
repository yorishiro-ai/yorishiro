//! Bulk import and embedding dispatch commit together on a Loco worker.

use loco_rs::{app::AppContext, bgworker::BackgroundWorker, prelude::async_trait};
use sea_orm::DatabaseTransaction;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    dispatch::{JobSpec, dispatch_job},
    embedding_sync::WorkerClass,
};
use crate::error::{ResultExt, YorishiroError};
use crate::models::{
    access_key_audit_log::{self, AuditAction, AuditActor},
    access_keys::AuthContext,
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ImportArgs {
    lifecycle_id: Option<Uuid>,
    tenant_id: Uuid,
    workspace_id: Uuid,
    access_key_id: Uuid,
    user_id: Option<Uuid>,
    jsonl: String,
    worker_class: WorkerClass,
}

pub(crate) struct ImportWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<ImportArgs> for ImportWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: ImportArgs) -> loco_rs::Result<()> {
        super::lifecycle::perform_with_terminal_lifecycle::<Self, _, _, _>(
            &self.ctx,
            args.lifecycle_id,
            args.worker_class,
            args.workspace_id,
            &args,
            || async { self.import(&args).await.map_err(loco_rs::Error::wrap) },
        )
        .await
    }
}

impl ImportWorker {
    async fn import(&self, args: &ImportArgs) -> Result<(), YorishiroError> {
        let id = args
            .lifecycle_id
            .ok_or_else(|| YorishiroError::Internal("import lifecycle missing".into()))?;
        // Hold control-plane permission stable until the workspace write commits.
        let authorization = crate::db::begin_authorization(&self.ctx.db)
            .await
            .internal()?;
        let txn =
            crate::db::begin_workspace_write(&self.ctx, args.tenant_id, args.workspace_id).await?;
        crate::db::lock_for_update(&txn, &format!("import:{id}"))
            .await
            .internal()?;
        if !access_key_audit_log::job_completed(
            &txn,
            args.workspace_id,
            id,
            AuditAction::BackgroundJobCompleted,
        )
        .await?
        {
            crate::models::iam::access::require_iam_action(
                authorization.as_ref().unwrap_or(&txn),
                &AuthContext {
                    tenant_id: args.tenant_id,
                    workspace_id: Some(args.workspace_id),
                    access_key_id: args.access_key_id,
                    user_id: args.user_id,
                },
                crate::models::iam::policy::Action::CreateImportJob,
                crate::models::iam::policy::Resource::Workspace {
                    tenant_id: args.tenant_id,
                    workspace_id: args.workspace_id,
                },
            )
            .await?;
            let quotas = crate::data::quotas::for_tenant(&self.ctx, args.tenant_id).await?;
            let result = crate::models::transfer::import::import_with_quota(
                &txn,
                args.tenant_id,
                args.workspace_id,
                args.user_id,
                args.jsonl.as_bytes(),
                quotas.entities,
            )
            .await?;
            for entity in &result.entity_ids {
                super::embedding_sync::enqueue_after_write(
                    &self.ctx,
                    args.workspace_id,
                    *entity,
                    &txn,
                )
                .await?;
            }
            access_key_audit_log::record(&txn, AuditActor {
                tenant_id: args.tenant_id, workspace_id: args.workspace_id,
                access_key_id: args.access_key_id, user_id: args.user_id,
            }, AuditAction::BackgroundJobCompleted, serde_json::json!({"job_id": id, "schemas": result.schemas, "entities": result.entities, "relations": result.relations})).await?;
        }
        txn.commit().await.internal()?;
        if let Some(authorization) = authorization {
            authorization.commit().await.internal()?;
        }
        Ok(())
    }
}

/// Opens the caller's write session, enqueues an import and commits.
///
/// # Errors
/// Returns the session, authorization or enqueue error.
pub(crate) async fn enqueue_for_actor(
    ctx: &AppContext,
    actor: AuthContext,
    jsonl: String,
) -> Result<Uuid, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::CreateImportJob,
        ),
        None,
        async |session| enqueue(ctx, session.txn(), &session.ctx, jsonl).await,
    )
    .await
}

pub(crate) async fn enqueue(
    ctx: &AppContext,
    txn: &DatabaseTransaction,
    auth: &AuthContext,
    jsonl: String,
) -> Result<Uuid, YorishiroError> {
    let authorization = crate::db::begin_authorization(&ctx.db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        authorization.as_ref().unwrap_or(txn),
        auth,
        crate::models::iam::policy::Action::CreateImportJob,
        crate::models::iam::policy::Resource::Workspace {
            tenant_id: auth.tenant_id,
            workspace_id: auth.require_workspace()?,
        },
    )
    .await?;
    let worker_class =
        crate::workers::target::resolve_worker_class(ctx, auth.require_workspace()?).await?;
    let args = ImportArgs {
        lifecycle_id: None,
        tenant_id: auth.tenant_id,
        workspace_id: auth.require_workspace()?,
        access_key_id: auth.access_key_id,
        user_id: auth.user_id,
        jsonl,
        worker_class,
    };
    let id = dispatch_job(
        ctx,
        JobSpec {
            job_name: "import",
            workspace_id: auth.require_workspace()?,
            class: worker_class,
            worker_name: ImportWorker::class_name(),
            dispatch_payload: serde_json::to_value(args).internal()?,
            use_queue_concurrency: true,
        },
        Some(txn),
    )
    .await
    .internal()?;
    if let Some(authorization) = authorization {
        authorization.commit().await.internal()?;
    }
    id.parse().internal()
}
