use sea_query::{Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::TenantRecord;

#[derive(Iden)]
enum Tenants {
    Table,
    Id,
    Name,
    MaxWorkspaces,
    CreatedAt,
}

/// Creates a tenant, enforcing the system-wide tenant cap from `YORISHIRO_MAX_TENANTS` (`0` or unset means unlimited).
/// This is a deployment-wide limit rather than a per-tenant column, since it bounds a deployment to a single tenant without needing a settings table: `yorishiro-server` defaults this to `1` (single-tenant) and deployments that want multiple tenants set it to `0` or a higher count.
/// It is enforced only in application code (there is no anti-tampering against an operator who edits the source or the env var directly), like the rest of this module's caps, it exists for product consistency, not as a security boundary against whoever controls the deployment.
pub async fn create_tenant(
    pool: &PgPool,
    name: &str,
    max_workspaces: Option<i32>,
) -> Result<TenantRecord, YorishiroError> {
    create_tenant_with_cap(pool, name, max_workspaces, max_tenants_from_env()?).await
}

/// Reads and parses `YORISHIRO_MAX_TENANTS`.
/// Unset or `0` means unlimited; a negative or non-integer value is a misconfiguration and fails loudly rather than silently falling back to unlimited.
pub fn max_tenants_from_env() -> Result<Option<i32>, YorishiroError> {
    match std::env::var("YORISHIRO_MAX_TENANTS") {
        Ok(raw) => {
            let parsed = raw.parse::<i32>().map_err(|_| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must be an integer, got '{raw}'"
                ))
            })?;
            match parsed {
                0 => Ok(None),
                n if n < 0 => Err(YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must not be negative, got '{raw}'"
                ))),
                n => Ok(Some(n)),
            }
        }
        Err(_) => Ok(None),
    }
}

/// Cap-checking logic factored out of `create_tenant` so tests can exercise it without mutating the process-wide `YORISHIRO_MAX_TENANTS` env var (which would race against other tests running concurrently in the same test binary).
///
/// `pub` (rather than private) only so the crate-root integration test in `tests/` can call it directly; `#[doc(hidden)]` keeps it out of the public API docs.
#[doc(hidden)]
pub async fn create_tenant_with_cap(
    pool: &PgPool,
    name: &str,
    max_workspaces: Option<i32>,
    max_tenants: Option<i32>,
) -> Result<TenantRecord, YorishiroError> {
    if let Some(max) = max_tenants {
        let (sql, values) = Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from((Alias::new("identity"), Tenants::Table))
            .build_sqlx(PostgresQueryBuilder);
        let (count,): (i64,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .internal()?;
        if count >= i64::from(max) {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "this deployment has reached its tenant limit ({max}, set via \
                     YORISHIRO_MAX_TENANTS); raise or unset that variable to create another tenant"
                ),
            });
        }
    }

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Tenants::Table))
        .columns([Tenants::Name, Tenants::MaxWorkspaces])
        .values_panic([name.into(), max_workspaces.into()])
        .returning(Query::returning().columns(tenant_columns()))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_one(pool)
        .await
        .internal()
}

fn tenant_columns() -> [Tenants; 4] {
    [
        Tenants::Id,
        Tenants::Name,
        Tenants::MaxWorkspaces,
        Tenants::CreatedAt,
    ]
}

/// Takes `&mut PgConnection` (rather than `&PgPool`, like most of this module) so a caller can compose it into a larger transaction: e.g. `add_member` calls this as part of its own atomic user-creation-plus-membership flow.
/// Pass `&mut pool.acquire().await?` for a standalone call.
pub async fn get_tenant(
    conn: &mut PgConnection,
    tenant_id: Uuid,
) -> Result<TenantRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(tenant_columns())
        .from((Alias::new("identity"), Tenants::Table))
        .and_where(Expr::col(Tenants::Id).eq(tenant_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))
}

pub async fn list_tenants(pool: &PgPool) -> Result<Vec<TenantRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(tenant_columns())
        .from((Alias::new("identity"), Tenants::Table))
        .order_by(Tenants::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_all(pool)
        .await
        .internal()
}

/// Updates a tenant's workspace cap.
/// `None` means unlimited.
/// Existing workspaces keep whatever `max_entities` they were created with: only newly created workspaces are affected by a change here, since retroactively shrinking a cap could put an existing workspace over its own limit.
pub async fn set_tenant_max_workspaces(
    pool: &PgPool,
    tenant_id: Uuid,
    max_workspaces: Option<i32>,
) -> Result<TenantRecord, YorishiroError> {
    let (sql, values) = Query::update()
        .table((Alias::new("identity"), Tenants::Table))
        .values([(Tenants::MaxWorkspaces, max_workspaces.into())])
        .and_where(Expr::col(Tenants::Id).eq(tenant_id))
        .returning(Query::returning().columns(tenant_columns()))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))
}

#[cfg(test)]
#[path = "../../../tests/repositories/tenancy/tenants.rs"]
mod tests;
