//! Raw sqlx connection handling that sits beside Loco's `sea_orm::DatabaseConnection`, not through it.
//!
//! Loco's own pool construction (`sea_orm::ConnectOptions`) has no `after_connect`/`after_release` hook, so the RLS session-state lifecycle this deployment depends on is built here as a standalone `sqlx::PgPool` and stored in `AppContext::shared_store` (see `Hooks::after_context` in `src/app.rs`).
//! That pool is also wrapped as a `sea_orm::DatabaseConnection`, which preserves its `after_connect` hook: the hook belongs to the sqlx pool, not to SeaORM's wrapper.
//!
//! Requests reach the database through `TenantDb::begin_for_workspace`, whose returned `DatabaseTransaction` carries both the entity API and raw SQL the entity layer can't express (JSONB containment, pgvector search, advisory locks).
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
    /// `after_connect` issues `SET ROLE` once per physical connection, so every query runs as `yorishiro_app`, which cannot bypass RLS; a failure to assume that role fails the connection rather than falling back to the connecting role.
    /// `after_release` resets the GUCs before a connection returns to the pool, covering `acquire_for_workspace`'s use outside any transaction.
    ///
    /// `connect_lazy`, not `connect`: `Hooks::after_context` (this function's only caller) runs before migrations create the `yorishiro_app` role, so connecting eagerly would fail every connection on a fresh database until `acquire_timeout` gives up.
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

    /// The same scoping as `begin_for_workspace`, on a bare connection rather than a transaction.
    ///
    /// `is_local=false` (session-level) is required here: this runs outside an explicit transaction, so `true` would discard the setting when the implicit single-statement transaction ends, leaving later queries on the connection unscoped.
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
/// Every `ActiveModelBehavior::before_save` for a `uuidv7_pk`-keyed entity assigns the result to `self.id` unconditionally: `NotSet` means "leave what the caller put there", so a caller that set `id` explicitly is respected.
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
/// Returns `Set(now)` for an update that has not named its own timestamp, and the value untouched otherwise, so a deliberate caller (a backfill, an import preserving original timestamps) is never overwritten.
///
/// Checks `is_set()` rather than `is_unchanged()`: an `ActiveModel` built with `..Default::default()` leaves untouched fields `NotSet`, which `is_unchanged()` does not match.
///
/// `insert` is a parameter because an insert normally takes the column's database default. `content_schemas` is the exception and passes `false` on both paths, since SQLite refuses a non-constant default on a column added to an existing table.
///
/// There is deliberately no counterpart for `created_at`: that column has `NOT NULL DEFAULT now()` on both backends, so nothing in application code should be able to move it.
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
/// `connect_timeout`: an intermittent failure under load with nothing pointing at the cause.
/// Rejecting boot is what turns that into a legible startup error.
pub fn require_min_sqlite_connections(max_connections: u32) -> Result<(), String> {
    if max_connections < 2 {
        return Err(format!(
            "database.max_connections is {max_connections}, but the SQLite backend requires at least 2: \
             an authenticated request holds one connection on its transaction while updating last_used_at \
             on a second. With only one, that second acquire waits out connect_timeout and fails, \
             surfacing as an intermittent 500 under load."
        ));
    }
    Ok(())
}

/// Serializes a transaction against others naming the same `key`, until it commits.
///
/// The lock is transaction-scoped, so it releases on commit or rollback without an unlock call to forget.
/// Takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice), since every caller already holds one via `Authorized::txn()`.
///
/// A no-op on SQLite, which has no named-lock primitive to substitute.
///
/// That is sound rather than merely convenient, because every caller here locks, reads a count or existence check, then writes within the same transaction gated on that read.
/// SQLite allows one write transaction at a time, so a transaction committing after another has written gets `SQLITE_BUSY` and fails whole; the TOCTOU this lock closes on Postgres surfaces as a retryable error rather than a silently-accepted inconsistent write.
/// A caller holding the lock for some other reason would need its own justification, and none does.
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

/// A guard that holds a session-scoped advisory lock on a workspace for reindex serialization.
///
/// On PostgreSQL, holds a detached connection whose session-end releases the lock.
/// On SQLite, is a no-op (empty guard).
///
/// `conn` is detached so dropping the guard ends the session, which is when PostgreSQL
/// releases session-level advisory locks. Using `detach` instead of returning the connection
/// to the pool is necessary: pool return keeps the session alive, so the lock would persist
/// and cause the next reindex to re-enter (advisory locks are per-session, not per-connection).
pub struct WorkspaceReindexLockGuard {
    #[allow(dead_code)]
    conn: Option<sqlx::PgConnection>,
}

impl Drop for WorkspaceReindexLockGuard {
    fn drop(&mut self) {
        // Detached connection drop ends the session; PostgreSQL releases
        // session-level advisory locks at session end.
    }
}

/// Acquires a session-scoped advisory lock on a workspace for reindex serialization.
///
/// On PostgreSQL, acquires `pg_advisory_lock` on a bare pooled connection and detaches it.
/// The lock is held until the guard is dropped, which ends the detached session and releases
/// the lock. Pool return keeps the session alive, so the lock would persist and cause the next
/// reindex to re-enter (advisory locks are per-session, not per-connection).
pub async fn acquire_workspace_reindex_lock(
    pool: PgPool,
    workspace_id: Uuid,
) -> Result<WorkspaceReindexLockGuard, sqlx::Error> {
    let mut conn = pool.acquire().await?;

    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(workspace_id.to_string())
        .execute(conn.as_mut())
        .await?;

    // Detach the connection so dropping the guard ends the session.
    // Session end is when PostgreSQL releases session-level advisory locks;
    // returning to the pool would keep the session (and the lock) alive.
    let detached = conn.detach();

    Ok(WorkspaceReindexLockGuard {
        conn: Some(detached),
    })
}

/// Acquires the session-scoped advisory lock and runs a reindex under it, returning the outcome.
///
/// This is the single-entry-point the concurrency test exercises: it races two such calls
/// against the same workspace with different providers so the lock's serialization — and the
/// restamp-on-full-success invariant that depends on it — can be verified.
pub async fn reindex_workspace_with_lock(
    pool: PgPool,
    workspace_id: Uuid,
    conn: &impl ConnectionTrait,
    candidate_ids: &[Uuid],
    provider: &dyn crate::services::embedding::EmbeddingProvider,
) -> Result<crate::services::embedding::sync::ReindexOutcome, crate::YorishiroError> {
    let lock = acquire_workspace_reindex_lock(Some(pool), workspace_id)
        .await
        .map_err(|err| {
            crate::YorishiroError::Internal(anyhow::anyhow!(
                "failed to acquire workspace lock: {err}"
            ))
        })?;
    let outcome = crate::services::embedding::sync::reindex_workspace(
        conn,
        workspace_id,
        candidate_ids,
        provider,
    )
    .await?;
    drop(lock);
    Ok(outcome)
}
