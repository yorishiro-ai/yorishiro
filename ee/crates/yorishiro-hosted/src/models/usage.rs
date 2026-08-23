//! Usage counters for invoicing/dashboard display.

use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};

#[derive(Debug, Clone, Serialize)]
pub struct TenantUsage {
    pub tenant_id: Uuid,
    pub workspace_count: i64,
    pub member_count: i64,
    pub entity_count: i64,
}

#[derive(FromQueryResult)]
struct Count {
    count: i64,
}

async fn fetch_count(
    conn: &impl ConnectionTrait,
    sql: &str,
    tenant_id: Uuid,
) -> Result<i64, YorishiroError> {
    let row = Count::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        [tenant_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;
    Ok(row.map(|r| r.count).unwrap_or(0))
}

/// Computes usage counters for invoicing/dashboard display.
/// Runs over `ctx.db` (the admin/migration-role connection), since it aggregates across every workspace in a tenant and `content_entities` only has a workspace-level RLS policy, not a tenant-wide one.
pub async fn compute_tenant_usage(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<TenantUsage, YorishiroError> {
    let workspace_count = fetch_count(
        conn,
        "SELECT COUNT(*) AS count FROM identity_workspaces WHERE tenant_id = $1",
        tenant_id,
    )
    .await?;

    let member_count = fetch_count(
        conn,
        "SELECT COUNT(*) AS count FROM identity_tenant_memberships WHERE tenant_id = $1",
        tenant_id,
    )
    .await?;

    let entity_count = fetch_count(
        conn,
        "SELECT COUNT(*) AS count FROM content_entities e \
         JOIN identity_workspaces w ON w.id = e.workspace_id \
         WHERE w.tenant_id = $1",
        tenant_id,
    )
    .await?;

    Ok(TenantUsage {
        tenant_id,
        workspace_count,
        member_count,
        entity_count,
    })
}
