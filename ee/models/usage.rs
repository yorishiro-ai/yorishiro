//! Usage counters for invoicing/dashboard display.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{
    entity_entities, tenant_memberships, workspace_workspaces,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct TenantUsage {
    pub tenant_id: Uuid,
    pub workspace_count: i64,
    pub member_count: i64,
    pub entity_count: i64,
}

/// Computes usage counters for invoicing/dashboard display.
/// Runs over `ctx.db` (the admin/migration-role connection), since it aggregates across every workspace in a tenant and `entity_entities` only has a workspace-level RLS policy, not a tenant-wide one.
pub async fn compute_tenant_usage(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<TenantUsage, YorishiroError> {
    let workspace_count = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    let member_count = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    // Filters on workspace_workspaces.tenant_id, a column entity_entities does not itself carry,
    // via the belongs_to relation entity_entities::Relation::IdentityWorkspaces already defines
    // (entity_entities.workspace_id -> workspace_workspaces.id).
    let entity_count = entity_entities::Entity::find()
        .inner_join(workspace_workspaces::Entity)
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    Ok(TenantUsage {
        tenant_id,
        workspace_count: workspace_count as i64,
        member_count: member_count as i64,
        entity_count: entity_count as i64,
    })
}
