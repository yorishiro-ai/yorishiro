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
    api_key_audit_log::{self, AuditAction, AuditActor},
    api_keys::AuthContext,
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ImportArgs {
    lifecycle_id: Option<Uuid>,
    tenant_id: Uuid,
    workspace_id: Uuid,
    api_key_id: Uuid,
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
            || async {
                self.import(&args)
                    .await
                    .map_err(|error| loco_rs::Error::Message(error.to_string()))
            },
        )
        .await
    }
}

impl ImportWorker {
    async fn import(&self, args: &ImportArgs) -> Result<(), YorishiroError> {
        let id = args
            .lifecycle_id
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("import lifecycle missing")))?;
        // Hold control-plane permission stable until the workspace write commits.
        let authorization =
            if self.ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
                Some(crate::db::begin_write(&self.ctx.db).await.internal()?)
            } else {
                None
            };
        let txn =
            crate::db::begin_workspace_write(&self.ctx, args.tenant_id, args.workspace_id).await?;
        crate::db::lock_for_update(&txn, &format!("import:{id}"))
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
            crate::models::access::require_iam_action(
                authorization.as_ref().unwrap_or(&txn),
                &AuthContext {
                    tenant_id: args.tenant_id,
                    workspace_id: Some(args.workspace_id),
                    api_key_id: args.api_key_id,
                    user_id: args.user_id,
                    scope: crate::models::api_keys::ApiKeyScope::Schema,
                    audit: false,
                },
                crate::models::api_keys::ApiKeyScope::Schema,
                crate::models::iam_policy::Action::ImportCreate,
                crate::models::iam_policy::Resource::Workspace {
                    tenant_id: args.tenant_id,
                    workspace_id: args.workspace_id,
                },
            )
            .await?;
            let quotas = crate::quotas(&self.ctx, args.tenant_id).await?;
            let result = crate::models::import::import_with_quota(
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
            api_key_audit_log::record(&txn, AuditActor {
                tenant_id: args.tenant_id, workspace_id: args.workspace_id,
                api_key_id: args.api_key_id, user_id: args.user_id,
            }, AuditAction::BackgroundJobCompleted, serde_json::json!({"job_id": id, "schemas": result.schemas, "entities": result.entities, "relations": result.relations})).await?;
        }
        txn.commit().await.internal()?;
        if let Some(authorization) = authorization {
            authorization.commit().await.internal()?;
        }
        Ok(())
    }
}

pub(crate) async fn enqueue(
    ctx: &AppContext,
    txn: &DatabaseTransaction,
    auth: &AuthContext,
    jsonl: String,
) -> Result<Uuid, YorishiroError> {
    let authorization = if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
        Some(crate::db::begin_write(&ctx.db).await.internal()?)
    } else {
        None
    };
    crate::models::access::require_iam_action(
        authorization.as_ref().unwrap_or(txn),
        auth,
        crate::models::api_keys::ApiKeyScope::Schema,
        crate::models::iam_policy::Action::ImportCreate,
        crate::models::iam_policy::Resource::Workspace {
            tenant_id: auth.tenant_id,
            workspace_id: auth.require_workspace()?,
        },
    )
    .await?;
    let worker_class =
        crate::controllers::extractors::resolve_worker_class(ctx, auth.require_workspace()?)
            .await
            .map_err(|error| error.0)?;
    let args = ImportArgs {
        lifecycle_id: None,
        tenant_id: auth.tenant_id,
        workspace_id: auth.require_workspace()?,
        api_key_id: auth.api_key_id,
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
