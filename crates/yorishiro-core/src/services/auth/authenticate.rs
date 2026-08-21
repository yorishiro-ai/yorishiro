use sea_query::{Expr, Query};
use sea_query_binder::SqlxBinder;
use uuid::Uuid;

use crate::db::{DbHandle, Engine};
use crate::error::{ResultExt, YorishiroError};

use super::{ApiKeyScope, ApiKeys, AuthContext, hash_key};

#[cfg(feature = "sqlite")]
#[derive(sea_query::Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
}

/// Verifies a presented raw API key and resolves the workspace, tenant, and scope it belongs to.
///
/// At this point neither the workspace nor the tenant is known yet (so RLS's `app.current_workspace`/`app.current_tenant` can't be set, on the engine that has them), which is why this takes the whole [`DbHandle`] rather than a scoped connection.
///
/// This is the rule this crate itself applies: a key resolves to the one workspace recorded on it.
/// A deployment needing a different rule supplies its own [`super::Authenticator`] rather than changing this function.
pub async fn authenticate(
    db: &DbHandle,
    presented_key: &str,
) -> Result<AuthContext, YorishiroError> {
    let key_hash = hash_key(presented_key);

    let row: Option<(Uuid, Uuid, Uuid, String, Option<Uuid>)> = match db {
        DbHandle::Postgres { tenant, .. } => {
            // Calling a SECURITY DEFINER function as the FROM-clause row source has no first-class sea-query form (it isn't a table, so `.from()` can't target it without falling back to `Expr::cust()`, which would just hide a raw SQL string inside a builder call rather than actually building the query).
            // This stays raw SQL for the same reason the session commands in `db.rs` do.
            // `identity.authenticate_api_key` is SECURITY DEFINER so this bypasses RLS on `api_keys`/`workspaces` for verification purposes only, and limits the columns it returns to id/workspace_id/tenant_id/scope (never the `key_hash` itself).
            // Runs on `tenant`'s pool, not `identity`'s: the migration's `REVOKE ALL ... GRANT EXECUTE TO yorishiro_app` grants this function to the role every request already connects as, and running it as the migration role instead would exercise a grant nothing checks.
            sqlx::query_as(
                "SELECT id, workspace_id, tenant_id, scope, user_id FROM identity.authenticate_api_key($1)",
            )
            .bind(key_hash)
            .fetch_optional(tenant.pool())
            .await
            .internal()?
        }
        #[cfg(feature = "sqlite")]
        DbHandle::Sqlite(sqlite) => {
            // No RLS to bypass here, so this is the plain join the Postgres function itself wraps: SECURITY DEFINER exists to reach past a policy that Sqlite never had in the first place.
            // Column order matches the tuple `authenticate` deserializes into: id, workspace_id, tenant_id, scope, user_id.
            let (sql, values) = Query::select()
                .columns([
                    (ApiKeys::Table, ApiKeys::Id),
                    (ApiKeys::Table, ApiKeys::WorkspaceId),
                ])
                .columns([(Workspaces::Table, Workspaces::TenantId)])
                .columns([(ApiKeys::Table, ApiKeys::Scope)])
                .columns([(ApiKeys::Table, ApiKeys::UserId)])
                .from(ApiKeys::Table)
                .inner_join(
                    Workspaces::Table,
                    Expr::col((Workspaces::Table, Workspaces::Id))
                        .equals((ApiKeys::Table, ApiKeys::WorkspaceId)),
                )
                .and_where(Expr::col((ApiKeys::Table, ApiKeys::KeyHash)).eq(key_hash))
                .build_sqlx(sea_query::SqliteQueryBuilder);

            sqlx::query_as_with(&sql, values)
                .fetch_optional(sqlite.pool())
                .await
                .internal()?
        }
    };

    let (api_key_id, workspace_id, tenant_id, scope_str, user_id) =
        row.ok_or(YorishiroError::Unauthenticated)?;
    let scope = ApiKeyScope::from_db_str(&scope_str).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown api key scope in database: {scope_str}"
        ))
    })?;

    Ok(AuthContext {
        api_key_id,
        workspace_id,
        tenant_id,
        scope,
        user_id,
    })
}

/// Records the API key's last-used timestamp.
/// This is a best-effort update that doesn't affect authentication outcomes, so callers don't need to fail the whole request if it errors.
///
/// Keyed on the API key's own id alone.
/// The id is already unique, so also filtering on a workspace only narrows the match, and would miss any key whose stored workspace is not the one the request resolved to.
pub async fn touch_last_used<C>(conn: &mut C, api_key_id: Uuid) -> Result<(), YorishiroError>
where
    C: Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
{
    let (sql, values) = Query::update()
        .table(C::schema_table("identity", ApiKeys::Table))
        .values([(ApiKeys::LastUsedAt, Expr::current_timestamp().into())])
        .and_where(Expr::col(ApiKeys::Id).eq(api_key_id))
        .build_sqlx(C::builder());

    sqlx::query_with(&sql, values)
        .execute(conn)
        .await
        .internal()?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/services/auth/authenticate.rs"]
mod tests;
