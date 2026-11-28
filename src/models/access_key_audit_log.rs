//! Records of workspace operations an operator needs an after-the-fact record of: who did it, when, against which workspace, and what.
//!
//! A new audited operation extends [`AuditAction`], whose values the `action` CHECK constraint must also allow.
//!
//! The table is append-only: `yorishiro_app` holds `SELECT, INSERT` on it and nothing else.

use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
pub use crate::models::_entities::access_key_audit_log::{ActiveModel, Entity, Model};
use crate::models::db_enum::db_enum;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
    }
}

/// A committed completion record makes redelivery a no-op without replacing original undo snapshots.
pub(crate) async fn job_completed(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    action: AuditAction,
) -> Result<bool, YorishiroError> {
    Ok(job_result(conn, workspace_id, job_id, action)
        .await?
        .is_some())
}

pub(crate) async fn job_result(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    job_id: Uuid,
    action: AuditAction,
) -> Result<Option<serde_json::Value>, YorishiroError> {
    use crate::models::_entities::access_key_audit_log::Column;
    use sea_orm::QuerySelect;
    use sea_orm::{
        ExprTrait,
        sea_query::{Alias, Expr, Func, extension::postgres::PgExpr},
    };
    let job = match conn.get_database_backend() {
        sea_orm::DatabaseBackend::Postgres => Expr::col(Column::Detail).cast_json_field("job_id"),
        _ => Func::cust(Alias::new("json_extract"))
            .args([Expr::col(Column::Detail), Expr::val("$.job_id")])
            .into(),
    };
    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .filter(Column::Action.eq(action.as_db_str()))
        .filter(job.eq(job_id.to_string()))
        .select_only()
        .column(Column::Detail)
        .into_tuple::<serde_json::Value>()
        .one(conn)
        .await
        .internal()
}

db_enum! {
    /// The closed set of operations this table records.
    /// It matches the `action` CHECK constraint string for string.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum AuditAction {
        /// `entity_entities::undo_job`: a batch undo, restoring every entity a job's snapshots cover.
        UndoMigrationJob = "undo_migration_job",
        /// `embedding::reindex_workspace`: an embedding reindex, replacing all vectors in a workspace with those from a new model.
        ReindexEmbeddings = "reindex_embeddings",
        /// `entity_entities::fill_defaults`: a batch fill of absent required fields.
        FillDefaults = "fill_defaults",
        /// A bulk worker's durable result, committed with its effects.
        BackgroundJobCompleted = "background_job_completed",
    }
}

/// The acting key, for [`record`]: what an audited operation attributes itself to.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "serialized audit identifiers must remain distinct from the records they identify"
)]
pub(crate) struct AuditActor {
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub access_key_id: Uuid,
    pub user_id: Option<Uuid>,
}

/// Appends one row.
///
/// Record on the transaction the operation's effect lands on, so a rollback loses both together.
///
/// The RLS policy is `USING`-only, which PostgreSQL also applies as the implicit `WITH CHECK` on an insert.
/// A `conn` must therefore have set `app.current_workspace` to `actor.workspace_id`, unless it is the migration role.
pub(crate) async fn record(
    conn: &impl ConnectionTrait,
    actor: AuditActor,
    action: AuditAction,
    detail: serde_json::Value,
) -> Result<(), YorishiroError> {
    tracing::info!(
        caller = %crate::models::iam::names::access_key_yrn(actor.tenant_id, actor.access_key_id),
        workspace = %crate::models::iam::policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id: actor.workspace_id,
        }
        .yrn(),
        action = action.as_db_str(),
        "audited operation",
    );
    let active = ActiveModel {
        workspace_id: ActiveValue::Set(actor.workspace_id),
        tenant_id: ActiveValue::Set(actor.tenant_id),
        access_key_id: ActiveValue::Set(Some(actor.access_key_id)),
        user_id: ActiveValue::Set(actor.user_id),
        action: ActiveValue::Set(action.as_db_str().to_string()),
        detail: ActiveValue::Set(detail),
        ..Default::default()
    };
    active.insert(conn).await.internal()?;
    Ok(())
}

/// The workspace's audit trail, most recent first, capped by the page limit.
///
/// # Errors
/// Returns an error if the query fails.
pub async fn list_for_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<Model>, YorishiroError> {
    use crate::models::_entities::access_key_audit_log::Column;

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_desc(Column::CreatedAt)
        .order_by_desc(Column::Id)
        .limit(page.limit())
        .offset(page.offset())
        .all(conn)
        .await
        .internal()
}

pub(crate) async fn list_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::access_keys::AuthContext,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<Model>, YorishiroError> {
    use crate::models::_entities::access_key_audit_log::Column;
    use crate::models::iam::{access::read_workspaces, policy::Action};
    let workspaces = read_workspaces(&ctx.db, actor, Action::ListWorkspaceAuditEvents).await?;
    if workspaces.is_empty() {
        return Err(YorishiroError::AccessDenied {
            message: format!(
                "no workspace permits {}",
                Action::ListWorkspaceAuditEvents.qualified()
            ),
            hint: "request an applicable IAM policy grant".into(),
        });
    }
    let txn = crate::db::begin_workspace_list(ctx, actor.tenant_id, &workspaces).await?;
    let records = if let [workspace] = workspaces.as_slice() {
        list_for_workspace(&txn, *workspace, page).await?
    } else {
        Entity::find()
            .filter(Column::WorkspaceId.is_in(workspaces))
            .filter(Column::TenantId.eq(actor.tenant_id))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(page.limit())
            .offset(page.offset())
            .all(&txn)
            .await
            .internal()?
    };
    txn.commit().await.internal()?;
    Ok(records)
}
