use sea_query::{Alias, Asterisk, Expr, Func, Iden, PostgresQueryBuilder, Query, SelectStatement};
use sea_query_binder::SqlxBinder;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::{ResultExt, YorishiroError};

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
}

#[derive(Iden)]
enum TenantMemberships {
    Table,
    TenantId,
}

#[derive(Iden)]
enum Entities {
    Table,
    WorkspaceId,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TenantUsage {
    pub tenant_id: Uuid,
    pub workspace_count: i64,
    pub member_count: i64,
    pub entity_count: i64,
}

/// Builds and runs a `SELECT COUNT(*) ...` statement, returning the single count it yields.
/// Shared by every counter `compute_tenant_usage` needs: each caller only differs in the `FROM`/`JOIN`/`WHERE` clauses of `query`, and `COUNT(*)` always yields exactly one row.
async fn fetch_count(pool: &PgPool, query: SelectStatement) -> Result<i64, YorishiroError> {
    let (sql, values) = query.build_sqlx(PostgresQueryBuilder);
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .internal()?;
    Ok(count)
}

/// Computes usage counters for invoicing/dashboard display.
/// Runs over the admin/migration-role pool (the same one `identity_pool` uses), since it aggregates across every workspace in a tenant and `content.entities` only has a workspace-level RLS policy, not a tenant-wide one.
pub async fn compute_tenant_usage(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<TenantUsage, YorishiroError> {
    let workspace_count = fetch_count(
        pool,
        Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from((Alias::new("identity"), Workspaces::Table))
            .and_where(Expr::col(Workspaces::TenantId).eq(tenant_id))
            .to_owned(),
    )
    .await?;

    let member_count = fetch_count(
        pool,
        Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from((Alias::new("identity"), TenantMemberships::Table))
            .and_where(Expr::col(TenantMemberships::TenantId).eq(tenant_id))
            .to_owned(),
    )
    .await?;

    let entity_count = fetch_count(
        pool,
        Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from((Alias::new("content"), Entities::Table))
            .inner_join(
                (Alias::new("identity"), Workspaces::Table),
                Expr::col((Workspaces::Table, Workspaces::Id))
                    .equals((Entities::Table, Entities::WorkspaceId)),
            )
            .and_where(Expr::col((Workspaces::Table, Workspaces::TenantId)).eq(tenant_id))
            .to_owned(),
    )
    .await?;

    Ok(TenantUsage {
        tenant_id,
        workspace_count,
        member_count,
        entity_count,
    })
}

#[cfg(test)]
#[path = "../../tests/services/usage.rs"]
mod tests;
