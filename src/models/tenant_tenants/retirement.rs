//! Tenant retirement inventory and fenced lifecycle operations.

use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{
    api_keys, entity_entities, entity_relations, queue_job_dispatch_outbox, queue_job_lifecycles,
    schema_schemas, template_templates, tenant_memberships, workspace_memberships,
    workspace_schema_forks, workspace_workspaces,
};

/// A secret-free snapshot, not approval for a later destructive operation.
#[derive(serde::Serialize)]
pub struct RetirementInventory {
    pub tenant_id: Uuid,
    pub tenant_name: String,
    pub workspace_ids: Vec<Uuid>,
    pub memberships: u64,
    pub shared_user_accounts: u64,
    /// External forks must be resolved explicitly before irreversible cleanup.
    pub external_workspace_forks: u64,
    pub workspace_grants: u64,
    pub api_keys: u64,
    pub jobs: u64,
    pub active_jobs: u64,
    pub dispatches: u64,
    pub entities: u64,
    pub relations: u64,
    pub schemas: u64,
    pub templates: u64,
    /// Identities are always retained, including accounts belonging to no other tenant.
    pub identity_policy: &'static str,
}

/// Disables the owner's tenant without deleting retained data or user identities.
/// Confirmation names the tenant UUID, not a workspace or a mutable display name.
///
/// # Errors
/// Rejects stale or non-owner credentials, mismatched confirmation and infrastructure tenants.
/// Database failures roll back disablement, key revocation, work cancellation and audit together.
pub async fn disable_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    confirmed_tenant_id: Uuid,
) -> Result<(), YorishiroError> {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};

    if confirmed_tenant_id != actor.tenant_id
        || confirmed_tenant_id == super::INFRASTRUCTURE_TENANT_ID
    {
        return Err(YorishiroError::ValidationFailed {
            message: "tenant retirement confirmation does not match the target".into(),
            details: vec![],
            hint: "confirm the non-infrastructure tenant UUID explicitly".into(),
        });
    }
    let txn = crate::db::begin_write(db).await.internal()?;
    let user_id = crate::models::access::require_current_tenant_owner(&txn, actor).await?;
    super::ActiveModel {
        id: Set(actor.tenant_id),
        disabled_at: Set(Some(crate::time::now().fixed_offset())),
        ..Default::default()
    }
    .update(&txn)
    .await
    .internal()?;
    api_keys::Entity::delete_many()
        .filter(api_keys::Column::TenantId.eq(actor.tenant_id))
        .exec(&txn)
        .await
        .internal()?;
    cancel_work(&txn, actor.tenant_id).await?;
    crate::models::tenant_retirement_events::ActiveModel {
        tenant_id: Set(actor.tenant_id),
        actor_user_id: Set(Some(user_id)),
        action: Set("disabled".into()),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .internal()?;
    txn.commit().await.internal()
}

/// Cancels retained background work for an already-disabled tenant.
/// The caller must commit this transaction together with disablement and credential revocation.
///
/// # Errors
/// Rejects active or infrastructure tenants and propagates database failures.
pub async fn cancel_work(
    txn: &sea_orm::DatabaseTransaction,
    tenant_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::query_embedding_requests;
    use sea_orm::sea_query::Expr;
    if tenant_id == super::INFRASTRUCTURE_TENANT_ID {
        return Err(YorishiroError::Unauthenticated);
    }
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    if super::Model::find_by_id(txn, tenant_id)
        .await?
        .disabled_at
        .is_none()
    {
        return Err(YorishiroError::Conflict {
            message: "disable the tenant before cancelling its work".into(),
        });
    }
    let workspaces = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .all(txn)
        .await
        .internal()?
        .into_iter()
        .map(|row| row.id)
        .collect::<Vec<_>>();
    let now = crate::time::now();
    queue_job_lifecycles::Entity::update_many()
        .col_expr(
            queue_job_lifecycles::Column::Status,
            Expr::value("cancelled"),
        )
        .col_expr(queue_job_lifecycles::Column::CancelledAt, Expr::value(now))
        .col_expr(queue_job_lifecycles::Column::UpdatedAt, Expr::value(now))
        .col_expr(
            queue_job_lifecycles::Column::LeaseUntil,
            Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
        )
        .col_expr(
            queue_job_lifecycles::Column::Error,
            Expr::value("tenant disabled"),
        )
        .filter(queue_job_lifecycles::Column::WorkspaceId.is_in(workspaces.clone()))
        .filter(queue_job_lifecycles::Column::Status.is_not_in(["completed", "cancelled"]))
        .exec(txn)
        .await
        .internal()?;
    queue_job_dispatch_outbox::Entity::delete_many()
        .filter(queue_job_dispatch_outbox::Column::WorkspaceId.is_in(workspaces.clone()))
        .exec(txn)
        .await
        .internal()?;
    query_embedding_requests::Entity::update_many()
        .col_expr(
            query_embedding_requests::Column::Status,
            Expr::value("failed"),
        )
        .col_expr(
            query_embedding_requests::Column::Error,
            Expr::value("tenant disabled"),
        )
        .col_expr(
            query_embedding_requests::Column::CompletedAt,
            Expr::value(now),
        )
        .col_expr(
            query_embedding_requests::Column::UpdatedAt,
            Expr::value(now),
        )
        .filter(query_embedding_requests::Column::WorkspaceId.is_in(workspaces))
        .filter(query_embedding_requests::Column::Status.eq("pending"))
        .exec(txn)
        .await
        .internal()?;
    Ok(())
}

/// Inventories one tenant without changing credentials, memberships, jobs or data.
/// This operator entry point requires a trusted identity connection, not a tenant-scoped pool.
///
/// # Errors
/// Rejects the infrastructure tenant and missing tenants, or propagates database failures.
pub async fn inventory(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<RetirementInventory, YorishiroError> {
    if tenant_id == super::INFRASTRUCTURE_TENANT_ID {
        return Err(YorishiroError::ValidationFailed {
            message: "the infrastructure tenant cannot be retired".into(),
            details: vec![],
            hint: "select a non-infrastructure tenant UUID".into(),
        });
    }
    let tenant = super::Model::find_by_id(conn, tenant_id).await?;
    let workspace_ids = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .order_by_asc(workspace_workspaces::Column::Id)
        .all(conn)
        .await
        .internal()?
        .into_iter()
        .map(|workspace| workspace.id)
        .collect::<Vec<_>>();
    let memberships = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;
    let user_ids = tenant_memberships::Entity::find()
        .select_only()
        .column(tenant_memberships::Column::UserId)
        .filter(tenant_memberships::Column::TenantId.eq(tenant_id))
        .into_tuple::<Uuid>()
        .all(conn)
        .await
        .internal()?;
    let shared_user_accounts = tenant_memberships::Entity::find()
        .select_only()
        .column(tenant_memberships::Column::UserId)
        .filter(tenant_memberships::Column::UserId.is_in(user_ids))
        .filter(tenant_memberships::Column::TenantId.ne(tenant_id))
        .distinct()
        .into_tuple::<Uuid>()
        .all(conn)
        .await
        .internal()?
        .len() as u64;
    let external_workspace_forks = workspace_schema_forks::Entity::find()
        .filter(workspace_schema_forks::Column::SourceWorkspaceId.is_in(workspace_ids.clone()))
        .filter(workspace_schema_forks::Column::TenantId.ne(tenant_id))
        .count(conn)
        .await
        .internal()?;
    let workspace_grants = workspace_memberships::Entity::find()
        .filter(workspace_memberships::Column::WorkspaceId.is_in(workspace_ids.clone()))
        .count(conn)
        .await
        .internal()?;
    let api_keys = api_keys::Entity::find()
        .filter(api_keys::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;
    let jobs_query = queue_job_lifecycles::Entity::find()
        .filter(queue_job_lifecycles::Column::WorkspaceId.is_in(workspace_ids.clone()));
    let jobs = jobs_query.clone().count(conn).await.internal()?;
    let active_jobs = jobs_query
        .filter(queue_job_lifecycles::Column::Status.is_in(["queued", "running", "retrying"]))
        .count(conn)
        .await
        .internal()?;
    let dispatches = queue_job_dispatch_outbox::Entity::find()
        .filter(queue_job_dispatch_outbox::Column::WorkspaceId.is_in(workspace_ids.clone()))
        .count(conn)
        .await
        .internal()?;
    let entities = entity_entities::Entity::find()
        .filter(entity_entities::Column::WorkspaceId.is_in(workspace_ids.clone()))
        .count(conn)
        .await
        .internal()?;
    let relations = entity_relations::Entity::find()
        .filter(entity_relations::Column::WorkspaceId.is_in(workspace_ids.clone()))
        .count(conn)
        .await
        .internal()?;
    let schemas = schema_schemas::Entity::find()
        .filter(schema_schemas::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;
    let templates = template_templates::Entity::find()
        .filter(template_templates::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;
    Ok(RetirementInventory {
        tenant_id,
        tenant_name: tenant.name,
        workspace_ids,
        memberships,
        shared_user_accounts,
        external_workspace_forks,
        workspace_grants,
        api_keys,
        jobs,
        active_jobs,
        dispatches,
        entities,
        relations,
        schemas,
        templates,
        identity_policy: "retain_all_user_accounts",
    })
}
