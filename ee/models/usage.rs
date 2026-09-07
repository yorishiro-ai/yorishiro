//! Usage counters for invoicing/dashboard display.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{content_entities, tenant_memberships, workspaces};
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
/// Runs over `ctx.db` (the admin/migration-role connection), since it aggregates across every workspace in a tenant and `content_entities` only has a workspace-level RLS policy, not a tenant-wide one.
pub async fn compute_tenant_usage(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<TenantUsage, YorishiroError> {
    let workspace_count = workspaces::Entity::find()
        .filter(workspaces::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    let member_count = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    // Filters on identity_workspaces.tenant_id, a column content_entities does not itself carry,
    // via the belongs_to relation content_entities::Relation::IdentityWorkspaces already defines
    // (content_entities.workspace_id -> identity_workspaces.id).
    let entity_count = content_entities::Entity::find()
        .inner_join(workspaces::Entity)
        .filter(workspaces::Column::TenantId.eq(tenant_id))
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
