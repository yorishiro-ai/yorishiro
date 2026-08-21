use sea_query::{Alias, Asterisk, Expr, Func, Iden, IntoIden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
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

/// Resolves `YORISHIRO_MAX_TENANTS` for the Sqlite engine, where the cap is pinned to `1` rather
/// than read as `max_tenants_from_env` reads it (design memo §8 項目5 段階2, §2.2a).
///
/// Unset resolves to `Some(1)`: `max_tenants_from_env`'s own unset-means-unlimited would make a
/// zero-config Sqlite deployment refuse its own first tenant, which contradicts the point of
/// making Sqlite the zero-setup trial engine.
/// Any explicit value other than `"1"` (including `"0"`, unlimited) is a startup configuration
/// error: the cap cannot be raised on this engine by setting the variable, since there is no
/// database-enforced isolation under it to raise the cap against (`db::Storage`'s doc comment).
///
/// Pure and synchronous so it can be tested without the process-wide env var lock every other
/// test touching `YORISHIRO_MAX_TENANTS` in this crate has to take; it takes the raw value as an
/// argument rather than reading the environment itself for the same reason.
pub fn max_tenants_for_sqlite(raw: Option<&str>) -> Result<i32, YorishiroError> {
    match raw {
        None => Ok(1),
        Some("1") => Ok(1),
        Some(v) => Err(YorishiroError::Internal(anyhow::anyhow!(
            "YORISHIRO_MAX_TENANTS is '{v}', but the Sqlite engine only ever allows a single \
             tenant (design memo §8 項目5 段階2); unset the variable, set it to 1, or switch \
             YORISHIRO_DATABASE_DRIVER to postgres for multi-tenant operation"
        ))),
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
    let mut conn = pool.acquire().await.internal()?;
    create_tenant_on(&mut *conn, name, max_workspaces, max_tenants).await
}

/// `create_tenant_with_cap` against a caller-supplied connection, so a bootstrap that also creates
/// a workspace and an owner can run the whole sequence in one transaction.
/// The cap check and the insert are not atomic against a concurrent creation on another
/// connection; that race predates this and is unchanged by taking a connection here.
pub async fn create_tenant_on<C>(
    conn: &mut C,
    name: &str,
    max_workspaces: Option<i32>,
    max_tenants: Option<i32>,
) -> Result<TenantRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (i64,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    TenantRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    if let Some(max) = max_tenants {
        let (sql, values) = Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from(C::schema_table("identity", Tenants::Table))
            .build_sqlx(C::builder());
        let (count,): (i64,) = sqlx::query_as_with(&sql, values)
            .fetch_one(&mut *conn)
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

    let (cols, vals) = crate::db::with_generated_id::<C, _>(
        Tenants::Id,
        vec![
            Tenants::Name.into_iden(),
            Tenants::MaxWorkspaces.into_iden(),
        ],
        vec![name.into(), max_workspaces.into()],
    );
    let (sql, values) = Query::insert()
        .into_table(C::schema_table("identity", Tenants::Table))
        .columns(cols)
        .values_panic(vals)
        .returning(Query::returning().columns(tenant_columns()))
        .build_sqlx(C::builder());

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_one(conn)
        .await
        .internal()
}

/// Refuses to proceed if the database already holds more than one tenant.
///
/// `create_tenant_on`'s own cap check (above) guards the creation path, but does nothing for a
/// `.db` file that already carries more than one tenant when the process starts: copied in from
/// elsewhere, or written before this guard existed.
/// This is that second check (design memo §8 項目5 段階2, §2.2a データ検証): a startup-time data
/// check with no `YORISHIRO_MAX_TENANTS` counterpart, since that variable only ever gated
/// creation.
///
/// `> 1`, not `>= 1`: exactly one tenant is the healthy single-tenant state this engine exists
/// to run, not itself a violation.
pub async fn refuse_if_multiple_tenants_exist<C>(conn: &mut C) -> Result<(), YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (i64,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .expr(Func::count(Expr::col(Asterisk)))
        .from(C::schema_table("identity", Tenants::Table))
        .build_sqlx(C::builder());
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;
    if count > 1 {
        return Err(YorishiroError::Internal(anyhow::anyhow!(
            "this database holds {count} tenants, but the Sqlite engine only ever allows a \
             single tenant (design memo §8 項目5 段階2); switch YORISHIRO_DATABASE_DRIVER to \
             postgres for multi-tenant operation"
        )));
    }
    Ok(())
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
pub async fn get_tenant<C>(conn: &mut C, tenant_id: Uuid) -> Result<TenantRecord, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    TenantRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .columns(tenant_columns())
        .from(C::schema_table("identity", Tenants::Table))
        .and_where(Expr::col(Tenants::Id).eq(tenant_id))
        .build_sqlx(C::builder());

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
#[path = "../../../tests/models/tenancy/tenants.rs"]
mod tests;
