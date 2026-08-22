//! Raw sqlx connection handling that sits beside Loco's `sea_orm::DatabaseConnection`,
//! not through it.
//!
//! Loco's own pool construction (`sea_orm::ConnectOptions`) has no `after_connect`/
//! `after_release` hook, so the RLS session-state lifecycle this deployment depends on
//! (`SET ROLE` per physical connection, `set_config(...)` per request) is built here as a
//! standalone `sqlx::PgPool` and stored in `AppContext::shared_store` (see
//! `Hooks::after_context` in `src/app.rs`).
//!
//! That pool is also wrapped as a `sea_orm::DatabaseConnection`
//! (`SqlxPostgresConnector::from_sqlx_postgres_pool`), which preserves the wrapped pool's own
//! `after_connect` hook: wrapping doesn't touch it, since the hook is a property of the sqlx
//! pool, not of SeaORM. `TenantDb::begin_for_workspace` begins a transaction on that wrapped
//! connection and sets `app.current_tenant`/`app.current_workspace` transaction-locally
//! (`set_config(..., true)`), so Postgres RLS policies see them for the rest of that
//! transaction and they vanish automatically at commit or rollback, no `after_release` reset
//! needed for the GUCs. The SeaORM entity API (`Entity::find()`, `ActiveModel::insert()`, ...)
//! runs directly on the returned `DatabaseTransaction`; raw SQL the entity layer can't express
//! (JSONB containment, pgvector search, advisory locks) runs on the same handle via
//! `execute_unprepared`/`execute_raw`. See
//! <https://github.com/yotsunagi/yorishiro/issues/221> for the full history of this design.
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
    /// A connection scoped to `tenant_id`/`workspace_id`, such that row-level security
    /// confines it to that workspace's rows.
    async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error>;

    /// The underlying pool, for the control-plane paths that connect as the migration role
    /// and so must not be scoped: signup, setup, the admin CLI.
    fn pool(&self) -> &sqlx::Pool<sqlx::Postgres>;
}

#[derive(Clone)]
pub struct TenantDb {
    pool: PgPool,
    orm: DatabaseConnection,
}

impl TenantDb {
    /// Wraps a raw pool as-is.
    /// Callers must separately guarantee that `app.current_tenant`/`app.current_workspace`
    /// get reset when a connection returns to the pool (use `connect` for production).
    pub fn new(pool: PgPool) -> Self {
        let orm = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        Self { pool, orm }
    }

    /// Builds the production pool.
    /// The `after_connect` hook issues `SET ROLE` once per physical connection so all
    /// subsequent queries run as the `yorishiro_app` role, which cannot bypass RLS.
    /// The `after_release` hook resets `app.current_tenant`/`app.current_workspace` before
    /// returning a connection to the pool, preventing one workspace's session state from
    /// leaking to whichever workspace borrows the connection next. Belt-and-suspenders
    /// alongside `begin_for_workspace`'s transaction-local `set_config`: a connection reused
    /// outside a transaction (`acquire_for_workspace`, below) still needs this.
    ///
    /// `connect_lazy`, not `connect`: `Hooks::after_context` (this function's only caller) runs
    /// before Loco's own `db::converge` applies migrations, and the migration is what creates
    /// the `yorishiro_app` role `after_connect` requires. Eagerly opening a physical connection
    /// here would run `SET ROLE` against a role that doesn't exist yet on a fresh database,
    /// failing every connection attempt until the pool's `acquire_timeout` gives up
    /// (`PoolTimedOut`, with no clearer error underneath it). `connect_lazy` defers opening any
    /// physical connection until first use, by which point converge has already run.
    /// Tolerating the `SET ROLE` failure instead (warn and continue as the connecting role) was
    /// rejected: that role can bypass RLS, and failing to assume `yorishiro_app` must fail the
    /// connection, not silently degrade into the exact blindness `FORCE ROW LEVEL SECURITY`
    /// exists to prevent (`.claude/rules/workspace-checklist.md` in `yorishiro-specs`).
    /// The trade-off: a bad `DATABASE_URL` or a role that never gets created no longer fails at
    /// boot, only on the first request that needs this pool. The identity pool below still
    /// connects eagerly (it has no role dependency), so a URL typo still fails fast; only the
    /// role's existence goes unverified until first use.
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

    /// Begins a transaction scoped to `tenant_id`/`workspace_id`: the unit of work for one
    /// RLS-scoped request. `app.current_tenant`/`app.current_workspace` are set
    /// transaction-locally (`set_config(..., true)`), so Postgres RLS policies see them for
    /// every statement run on the returned transaction, entity API and raw SQL alike, and they
    /// disappear automatically at commit or rollback.
    ///
    /// The caller owns the transaction's lifetime: a write handler must call
    /// `txn.commit().await` explicitly, or every write in it is silently discarded when the
    /// transaction drops.
    pub async fn begin_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<DatabaseTransaction, DbErr> {
        let txn = self.orm.begin().await?;

        #[cfg(test)]
        txn.execute_unprepared("SET ROLE yorishiro_app").await?;

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

    /// Sets the session variables `app.current_tenant` and `app.current_workspace` on this
    /// connection so RLS can isolate both the tenant-level control-plane rows and the
    /// workspace-scoped content rows.
    ///
    /// Using `is_local=false` (session-level) matters: `is_local=true` would be discarded as
    /// soon as the implicit single-statement transaction ends when called outside an
    /// explicit transaction, causing later queries to see `current_setting(...)` as an empty
    /// string, i.e. isolation breaks.
    pub async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;

        #[cfg(test)]
        sqlx::query("SET ROLE yorishiro_app")
            .execute(conn.as_mut())
            .await?;
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
/// Carries two pools for the same reason the pre-rebuild `AppState` did: `identity` connects
/// with the migration role, bypassing RLS for the control-plane tables
/// (`identity_users`/`identity_tenant_memberships`/`identity_invites`) that have no
/// tenant/workspace context yet to scope by.
#[derive(Clone)]
pub struct DbHandle {
    pub tenant: TenantDb,
    pub identity: PgPool,
}

/// Serializes a transaction against others naming the same `key`, until it commits.
///
/// The lock is transaction-scoped, so it releases on commit or rollback without an unlock
/// call to forget. Takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in
/// practice), since every caller already holds one via `Authorized::txn()`.
pub async fn lock_for_update(conn: &impl ConnectionTrait, key: &str) -> Result<(), DbErr> {
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [key.into()],
    ))
    .await?;
    Ok(())
}

/// A held advisory lock, released when dropped or when its connection returns to the pool.
///
/// The lock lives on the connection rather than a transaction, so holding the connection is
/// what holds the lock, and the guard exists to make dropping it release the lock rather than
/// leaving it until the connection is recycled.
///
/// SeaORM's `DatabaseConnection` re-acquires a connection from the pool on every call, so this
/// pattern has no home there; it stays on the raw `sqlx::PgPool` permanently regardless of the
/// query layer above it (see #221's investigation).
pub struct SessionLock {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
    key: String,
}

impl SessionLock {
    /// Blocks until the lock is held, then keeps it until [`release`](Self::release) or drop.
    ///
    /// **No timeout, deliberately.** `pg_advisory_lock` waits indefinitely, and a timeout
    /// here would turn "wait your turn" into "skip the ordering check", which is the thing
    /// being protected.
    pub async fn acquire(pool: &PgPool, key: &str) -> Result<Self, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(key)
            .execute(&mut *conn)
            .await?;
        Ok(Self {
            conn,
            key: key.to_string(),
        })
    }

    /// The connection the lock is held on.
    pub fn conn(&mut self) -> &mut sqlx::PgConnection {
        &mut self.conn
    }

    /// Releases the lock and reports whether that failed.
    pub async fn release(mut self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(&self.key)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }
}
