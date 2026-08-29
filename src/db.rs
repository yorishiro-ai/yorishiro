//! Raw sqlx connection handling that sits beside Loco's `sea_orm::DatabaseConnection`, not through it.
//!
//! Loco's own pool construction (`sea_orm::ConnectOptions`) has no `after_connect`/`after_release` hook, so the RLS session-state lifecycle this deployment depends on (`SET ROLE` per physical connection, `set_config(...)` per request) is built here as a standalone `sqlx::PgPool` and stored in `AppContext::shared_store` (see `Hooks::after_context` in `src/app.rs`).
//!
//! That pool is also wrapped as a `sea_orm::DatabaseConnection` (`SqlxPostgresConnector::from_sqlx_postgres_pool`), which preserves the wrapped pool's own `after_connect` hook: wrapping doesn't touch it, since the hook is a property of the sqlx pool, not of SeaORM.
//! `TenantDb::begin_for_workspace` begins a transaction on that wrapped connection and sets `app.current_tenant`/`app.current_workspace` transaction-locally (`set_config(..., true)`), so Postgres RLS policies see them for the rest of that transaction and they vanish automatically at commit or rollback, no `after_release` reset needed for the GUCs.
//! The SeaORM entity API (`Entity::find()`, `ActiveModel::insert()`, ...) runs directly on the returned `DatabaseTransaction`; raw SQL the entity layer can't express (JSONB containment, pgvector search, advisory locks) runs on the same handle via `execute_unprepared`/`execute_raw`.
use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr, Statement, TransactionTrait,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// Where the deployment's data lives, for tenant-scoped request handling.
#[async_trait]
pub trait Storage: Send + Sync {
    /// A connection scoped to `tenant_id`/`workspace_id`, such that row-level security confines it to that workspace's rows.
    async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error>;

    /// The underlying pool, for the control-plane paths that connect as the migration role and so must not be scoped: signup, setup, the admin CLI.
    fn pool(&self) -> &sqlx::Pool<sqlx::Postgres>;
}

#[derive(Clone)]
pub struct TenantDb {
    pool: PgPool,
    orm: DatabaseConnection,
}

impl TenantDb {
    /// Wraps a raw pool as-is.
    /// Callers must separately guarantee that `app.current_tenant`/`app.current_workspace` get reset when a connection returns to the pool (use `connect` in production).
    pub fn new(pool: PgPool) -> Self {
        let orm = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        Self { pool, orm }
    }

    /// Builds the production pool.
    /// The `after_connect` hook issues `SET ROLE` once per physical connection so all subsequent queries run as the `yorishiro_app` role, which cannot bypass RLS.
    /// The `after_release` hook resets `app.current_tenant`/`app.current_workspace` before returning a connection to the pool, preventing one workspace's session state from leaking to whichever workspace borrows the connection next: belt-and-suspenders alongside `begin_for_workspace`'s transaction-local `set_config`, since a connection reused outside a transaction (`acquire_for_workspace`, below) still needs this.
    ///
    /// `connect_lazy`, not `connect`: `Hooks::after_context` (this function's only caller) runs before Loco's own `db::converge` applies migrations, and the migration is what creates the `yorishiro_app` role `after_connect` requires.
    /// Connecting eagerly here would run `SET ROLE` against a role that doesn't exist yet on a fresh database, failing every connection until `acquire_timeout` gives up.
    /// Failing to assume `yorishiro_app` must fail the connection outright rather than falling back to the connecting role, which can bypass RLS.
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET ROLE yorishiro_app")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .after_release(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("RESET app.current_tenant")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("RESET app.current_workspace")
                        .execute(&mut *conn)
                        .await?;
                    Ok(true)
                })
            })
            .connect_lazy(database_url)?;
        Ok(Self::new(pool))
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Begins a transaction scoped to `tenant_id`/`workspace_id`: the unit of work for one RLS-scoped request.
    /// `app.current_tenant`/`app.current_workspace` are set transaction-locally (`set_config(..., true)`), so Postgres RLS policies see them for every statement run on the returned transaction, entity API and raw SQL alike, and they disappear automatically at commit or rollback.
    ///
    /// The caller owns the transaction's lifetime: a write handler must call `txn.commit().await` explicitly, or every write in it is silently discarded when the transaction drops.
    pub async fn begin_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<DatabaseTransaction, DbErr> {
        let txn = self.orm.begin().await?;

        txn.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT set_config('app.current_tenant', $1, true)",
            [tenant_id.to_string().into()],
        ))
        .await?;
        txn.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT set_config('app.current_workspace', $1, true)",
            [workspace_id.to_string().into()],
        ))
        .await?;

        Ok(txn)
    }

    /// Sets the session variables `app.current_tenant` and `app.current_workspace` on this connection so RLS can isolate both the tenant-level control-plane rows and the workspace-scoped content rows.
    ///
    /// `is_local=false` (session-level) is required here: `is_local=true` would be discarded as soon as the implicit single-statement transaction ends, since this runs outside an explicit transaction, breaking isolation for later queries on the connection.
    pub async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;

        sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
            .bind(tenant_id.to_string())
            .execute(conn.as_mut())
            .await?;
        sqlx::query("SELECT set_config('app.current_workspace', $1, false)")
            .bind(workspace_id.to_string())
            .execute(conn.as_mut())
            .await?;
        Ok(conn)
    }
}

#[async_trait]
impl Storage for TenantDb {
    async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
        TenantDb::acquire_for_workspace(self, tenant_id, workspace_id).await
    }

    fn pool(&self) -> &sqlx::Pool<sqlx::Postgres> {
        TenantDb::pool(self)
    }
}

/// Which pools this deployment holds for control-plane vs. tenant-scoped access.
///
/// `identity` connects with the migration role, bypassing RLS for the control-plane tables (`identity_users`/`identity_tenant_memberships`/`identity_invites`) that have no tenant/workspace context yet to scope by.
#[derive(Clone)]
pub struct DbHandle {
    pub tenant: TenantDb,
    pub identity: PgPool,
}

/// A UUIDv7 for a primary key `before_save` hook to set on SQLite, or `ActiveValue::NotSet` to leave the column alone.
///
/// PostgreSQL's `id UUID PRIMARY KEY DEFAULT uuidv7()` (see `migration/src/helpers.rs::uuidv7_pk`) has no SQLite equivalent, so on that backend every insert must supply its own id or hit `NOT NULL constraint failed`.
/// Every `ActiveModelBehavior::before_save` for a `uuidv7_pk`-keyed entity calls this and assigns the result to `self.id` unconditionally: the `NotSet` case is exactly "leave whatever the caller already put there," so it is always safe to assign, not just on the SQLite branch.
/// Callers that set `id` explicitly (e.g. `ee/`'s official-templates publisher inserting a fixed nil-UUID infrastructure tenant) are respected because this only fires when the field is still unset.
pub fn sqlite_generated_id(
    conn: &impl ConnectionTrait,
    current: sea_orm::ActiveValue<Uuid>,
) -> sea_orm::ActiveValue<Uuid> {
    if current.is_not_set() && conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        sea_orm::ActiveValue::Set(Uuid::now_v7())
    } else {
        current
    }
}

/// The `updated_at` value a `before_save` hook should carry into an update, given what the caller already set.
///
/// Returns `Set(now)` for an update that has not named its own timestamp, and the value untouched otherwise.
/// Every table with an application-maintained `updated_at` calls this, so the rule is written once rather than seven times.
///
/// Checks `is_set()` rather than `is_unchanged()`: an `ActiveModel` built with `..Default::default()` leaves untouched fields `NotSet`, not `Unchanged`, and `is_unchanged()` only matches the latter.
/// A caller that sets the column deliberately (a backfill, an import preserving original timestamps) is never overwritten.
///
/// `insert` is a parameter rather than an assumption because the two cases genuinely differ: an insert takes the column's own database default, except where there is none.
/// `content_schemas` is that exception and passes `false` here on both paths, since SQLite refuses a non-constant default on a column added to an existing table; see its own `before_save`.
///
/// There is no counterpart for `created_at`, and its absence is the point rather than an omission: `migration/src/helpers.rs::created_at` gives that column `NOT NULL DEFAULT now()` on both backends, so an insert already carries the right value and nothing in application code should be able to move it.
/// `updated_at` needs this only because the value has to change on every later write, which a column default cannot express.
pub fn stamped_updated_at(
    insert: bool,
    current: sea_orm::ActiveValue<chrono::DateTime<chrono::FixedOffset>>,
) -> sea_orm::ActiveValue<chrono::DateTime<chrono::FixedOffset>> {
    if !insert && !current.is_set() {
        sea_orm::ActiveValue::Set(chrono::Utc::now().into())
    } else {
        current
    }
}

/// Rejects boot outright when `database.max_connections` is below 2, on SQLite only.
///
/// An `Authorized<R>`/`AuditAuthorized` request needs two connections at once: one held by its
/// transaction for the request's lifetime, and a second for `touch_last_used_at`, which cannot share
/// the transaction because a read-only handler drops it uncommitted and the update would roll back
/// with it.
///
/// At `max_connections: 1` the second acquire waits for the first to free, which happens only when
/// the request ends, so it always times out. Reads still answer `200` (that failure is logged and
/// swallowed), but any handler needing a real second connection fails with `500` after
/// `connect_timeout` — an intermittent failure under load with nothing pointing at the cause.
/// Rejecting boot is what turns that into a legible startup error.
pub fn require_min_sqlite_connections(max_connections: u32) -> Result<(), String> {
    if max_connections < 2 {
        return Err(format!(
            "database.max_connections is {max_connections}, but the SQLite backend requires at least 2: \
             an Authorized<R>/AuditAuthorized request holds one connection open on a transaction for its \
             duration while touching last_used_at on a second, independent connection from the same pool \
             (kept separate so a read-only handler that never commits doesn't silently roll that update back \
             with it, matching PostgreSQL's own authorize/touch_last_used_on split). \
             With only 1 connection available, that second acquire can only wait for connect_timeout and then \
             fail with a pool-timeout error, which surfaces as an intermittent 500 on any handler that itself \
             needs a second connection (read-only handlers still return normally, since the last_used_at \
             update is best-effort)."
        ));
    }
    Ok(())
}

/// Serializes a transaction against others naming the same `key`, until it commits.
///
/// The lock is transaction-scoped, so it releases on commit or rollback without an unlock call to forget.
/// Takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice), since every caller already holds one via `Authorized::txn()`.
///
/// A no-op on SQLite, not `pg_advisory_xact_lock`'s SQLite equivalent, because SQLite has none: `sea_orm::DatabaseBackend::Sqlite` in `execute_raw` would try to prepare `pg_advisory_xact_lock` as SQLite SQL and fail outright, and there is no comparable named-lock primitive to substitute.
/// This is sound, not merely convenient, for every caller of `lock_for_update` in this codebase: each one locks, reads a count or existence check, then writes (one statement or several, all within the same transaction) gated on that read.
/// SQLite allows only one write transaction to be in progress at a time; a transaction that read a value here and then tries to commit after a different transaction has since written and committed gets `SQLITE_BUSY` and the whole transaction fails, rather than being allowed to commit — one statement or all of them — against its now-stale read.
/// The TOCTOU this lock closes on Postgres therefore surfaces as a retryable error on SQLite instead of as a silently-accepted inconsistent write, which is the property the lock exists to guarantee, not a weaker substitute for it; a multi-write caller is covered by the same argument because SQLite's transaction is all-or-nothing, not because each of its writes is individually re-checked.
/// A caller that held the lock for a reason other than gating a commit on a prior read within the same transaction would not be covered by this reasoning and would need its own no-op justification; no caller in this codebase does that as of this writing.
pub async fn lock_for_update(conn: &impl ConnectionTrait, key: &str) -> Result<(), DbErr> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return Ok(());
    }
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [key.into()],
    ))
    .await?;
    Ok(())
}
