use sea_query::{Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::get_tenant;
use super::memberships::TenantMemberships;
use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::WorkspaceRecord;

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
    Name,
    MaxEntities,
    SchemaId,
    CreatedAt,
}

/// Creates a workspace under `tenant_id`, enforcing the tenant's `max_workspaces` cap. `NULL`
/// means unlimited, which is the default so self-hosted deployments are never capped unless an
/// operator explicitly sets a limit.
pub async fn create_workspace(
    pool: &PgPool,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
) -> Result<WorkspaceRecord, YorishiroError> {
    let mut conn = pool.acquire().await.internal()?;
    let tenant = get_tenant(&mut conn, tenant_id).await?;

    if let Some(max) = tenant.max_workspaces {
        let (sql, values) = Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from((Alias::new("identity"), Workspaces::Table))
            .and_where(Expr::col(Workspaces::TenantId).eq(tenant_id))
            .build_sqlx(PostgresQueryBuilder);
        let (count,): (i64,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .internal()?;
        if count >= i64::from(max) {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "tenant '{tenant_id}' has reached its workspace limit ({max}); \
                     raise max_workspaces or delete an existing workspace"
                ),
            });
        }
    }

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Workspaces::Table))
        .columns([
            Workspaces::TenantId,
            Workspaces::Name,
            Workspaces::MaxEntities,
            Workspaces::SchemaId,
        ])
        .values_panic([
            tenant_id.into(),
            name.into(),
            max_entities.into(),
            schema_id.into(),
        ])
        .returning(Query::returning().columns(workspace_columns()))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_one(pool)
        .await
        .internal()
}

fn workspace_columns() -> [Workspaces; 6] {
    [
        Workspaces::Id,
        Workspaces::TenantId,
        Workspaces::Name,
        Workspaces::MaxEntities,
        Workspaces::SchemaId,
        Workspaces::CreatedAt,
    ]
}

pub async fn list_workspaces(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<WorkspaceRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(workspace_columns())
        .from((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::TenantId).eq(tenant_id))
        .order_by(Workspaces::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_all(pool)
        .await
        .internal()
}

/// Every workspace `user_id` can log into, across all of their tenant memberships -- used to
/// resolve `POST /auth/login`'s `workspace_id` automatically when the caller omits it and the
/// answer is unambiguous (see `rest::identity::login`).
pub async fn list_workspaces_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<WorkspaceRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            (Workspaces::Table, Workspaces::Id),
            (Workspaces::Table, Workspaces::TenantId),
            (Workspaces::Table, Workspaces::Name),
            (Workspaces::Table, Workspaces::MaxEntities),
            (Workspaces::Table, Workspaces::SchemaId),
            (Workspaces::Table, Workspaces::CreatedAt),
        ])
        .from((Alias::new("identity"), Workspaces::Table))
        .inner_join(
            (Alias::new("identity"), TenantMemberships::Table),
            Expr::col((TenantMemberships::Table, TenantMemberships::TenantId))
                .equals((Workspaces::Table, Workspaces::TenantId)),
        )
        .and_where(Expr::col((TenantMemberships::Table, TenantMemberships::UserId)).eq(user_id))
        .order_by((Workspaces::Table, Workspaces::CreatedAt), Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_all(pool)
        .await
        .internal()
}

pub async fn get_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<WorkspaceRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(workspace_columns())
        .from((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?
        .ok_or_else(|| {
            YorishiroError::not_found(format!("workspace '{workspace_id}' was not found"))
        })
}

/// Resolves the `tenant_id` a workspace belongs to. Schema repository functions (and other
/// tenant-scoped queries) take `tenant_id` rather than `workspace_id` since the tenant-scoped
/// schema refactor, so callers that only have a `workspace_id` in hand (e.g. an entity/relation
/// repository function) use this to bridge the two. Takes a `PgConnection` (rather than the
/// `PgPool` the rest of this module uses) so it can be called from within an existing
/// transaction/connection instead of checking out a second one from the pool.
pub async fn resolve_tenant_id(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT tenant_id FROM identity.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut *conn)
            .await
            .internal()?;
    match row {
        Some((tenant_id,)) => Ok(tenant_id),
        None => Err(YorishiroError::not_found(format!(
            "workspace '{workspace_id}' was not found"
        ))),
    }
}

/// Deletes a workspace and everything under it. `identity.workspaces`'s foreign keys from
/// `content.entities`/`content.relations`/`content.schemas`/`identity.api_keys` are all
/// `ON DELETE CASCADE` (see the initial migration), so this one statement is enough --
/// callers don't need to delete those rows themselves first.
pub async fn delete_workspace(pool: &PgPool, workspace_id: Uuid) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        Err(YorishiroError::not_found(format!(
            "workspace '{workspace_id}' was not found"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/repositories/tenancy/workspaces.rs"]
mod tests;

/// Points a workspace at the schema it uses.
///
/// A schema is created against a workspace, so the workspace already exists by the time there
/// is a schema to link -- this closes the loop the other way round. Applying a template runs
/// both halves: create the schema for the workspace, then set it as the workspace's own.
pub async fn set_workspace_schema(
    pool: &PgPool,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::update()
        .table((Alias::new("identity"), Workspaces::Table))
        .value(Workspaces::SchemaId, schema_id)
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        return Err(YorishiroError::not_found(format!(
            "workspace '{workspace_id}' was not found"
        )));
    }
    Ok(())
}
