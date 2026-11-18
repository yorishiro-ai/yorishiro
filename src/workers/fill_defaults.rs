//! Durable schema-default migration, executed by the selected Loco queue provider.

use loco_rs::{app::AppContext, bgworker::BackgroundWorker, prelude::async_trait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{api_key_audit_log, entity_entities};
use crate::workers::{
    dispatch::{JobSpec, dispatch_job},
    embedding_sync::WorkerClass,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct FillDefaultsArgs {
    pub(crate) lifecycle_id: Option<Uuid>,
    pub(crate) tenant_id: Uuid,
    pub(crate) workspace_id: Uuid,
    pub(crate) api_key_id: Uuid,
    pub(crate) user_id: Option<Uuid>,
    pub(crate) schema_name: String,
    pub(crate) worker_class: WorkerClass,
}

pub struct FillDefaultsWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<FillDefaultsArgs> for FillDefaultsWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, args: FillDefaultsArgs) -> loco_rs::Result<()> {
        crate::workers::lifecycle::perform_with_terminal_lifecycle::<Self, _, _, _>(
            &self.ctx,
            args.lifecycle_id,
            args.worker_class,
            args.workspace_id,
            &args,
            || self.fill(&args),
        )
        .await
    }
}

impl FillDefaultsWorker {
    async fn fill(&self, args: &FillDefaultsArgs) -> loco_rs::Result<()> {
        let job_id = args
            .lifecycle_id
            .ok_or_else(|| loco_rs::Error::Message("fill-defaults lifecycle missing".into()))?;
        // Keep control-plane authorization fenced until the workspace mutation commits.
        let authorization =
            if self.ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
                Some(crate::db::begin_write(&self.ctx.db).await?)
            } else {
                None
            };
        let txn = crate::db::begin_workspace_write(&self.ctx, args.tenant_id, args.workspace_id)
            .await
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
        crate::db::lock_for_update(&txn, &format!("fill-defaults:{job_id}"))
            .await
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
        if api_key_audit_log::job_completed(
            &txn,
            args.workspace_id,
            job_id,
            api_key_audit_log::AuditAction::FillDefaults,
        )
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?
        {
            txn.commit().await?;
            return Ok(());
        }
        crate::models::access::require_iam_action(
            authorization.as_ref().unwrap_or(&txn),
            &crate::models::api_keys::AuthContext {
                tenant_id: args.tenant_id,
                workspace_id: Some(args.workspace_id),
                api_key_id: args.api_key_id,
                user_id: args.user_id,
                scope: crate::models::api_keys::ApiKeyScope::Migration,
                audit: false,
            },
            crate::models::api_keys::ApiKeyScope::Migration,
            crate::models::iam_policy::Action::MigrationFillDefaults,
            crate::models::iam_policy::Resource::Workspace {
                tenant_id: args.tenant_id,
                workspace_id: args.workspace_id,
            },
        )
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
        let report = entity_entities::fill_defaults(
            &txn,
            args.workspace_id,
            &args.schema_name,
            job_id,
            args.user_id,
        )
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
        api_key_audit_log::record(
            &txn,
            api_key_audit_log::AuditActor {
                tenant_id: args.tenant_id, workspace_id: args.workspace_id,
                api_key_id: args.api_key_id, user_id: args.user_id,
            },
            api_key_audit_log::AuditAction::FillDefaults,
            serde_json::json!({"schema_name": args.schema_name, "job_id": job_id,
                "entities_updated": report.entities_updated, "fields_filled": report.fields_filled}),
        ).await.map_err(|error| loco_rs::Error::Message(error.to_string()))?;
        txn.commit().await?;
        if let Some(authorization) = authorization {
            authorization.commit().await?;
        }
        Ok(())
    }
}

pub(crate) async fn enqueue(
    ctx: &AppContext,
    txn: &sea_orm::DatabaseTransaction,
    auth: &crate::models::api_keys::AuthContext,
    schema_name: String,
) -> Result<Uuid, crate::YorishiroError> {
    use crate::error::ResultExt;
    let authorization = if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
        Some(crate::db::begin_write(&ctx.db).await.internal()?)
    } else {
        None
    };
    crate::models::access::require_iam_action(
        authorization.as_ref().unwrap_or(txn),
        auth,
        crate::models::api_keys::ApiKeyScope::Migration,
        crate::models::iam_policy::Action::MigrationFillDefaults,
        crate::models::iam_policy::Resource::Workspace {
            tenant_id: auth.tenant_id,
            workspace_id: auth.require_workspace()?,
        },
    )
    .await?;
    // Validate against the caller's scoped transaction before accepting a durable request.
    crate::models::schema_schemas::get_active_schema(txn, auth.require_workspace()?, &schema_name)
        .await?;
    let worker_class =
        crate::controllers::extractors::resolve_worker_class(ctx, auth.require_workspace()?)
            .await
            .map_err(|error| error.0)?;
    let args = FillDefaultsArgs {
        lifecycle_id: None,
        tenant_id: auth.tenant_id,
        workspace_id: auth.require_workspace()?,
        api_key_id: auth.api_key_id,
        user_id: auth.user_id,
        schema_name,
        worker_class,
    };
    let id = dispatch_job(
        ctx,
        JobSpec {
            job_name: "fill_defaults",
            workspace_id: auth.require_workspace()?,
            class: worker_class,
            worker_name: FillDefaultsWorker::class_name(),
            dispatch_payload: serde_json::to_value(args)
                .map_err(|error| crate::YorishiroError::Internal(error.into()))?,
            use_queue_concurrency: true,
        },
        Some(txn),
    )
    .await
    .map_err(|error| crate::YorishiroError::Internal(error.into()))?;
    if let Some(authorization) = authorization {
        authorization.commit().await.internal()?;
    }
    id.parse()
        .map_err(|error| crate::YorishiroError::Internal(anyhow::Error::new(error)))
}
