//! Raw sqlx connection handling that sits beside Loco's `sea_orm::DatabaseConnection`,
//! not through it.
//!
//! Loco's own pool construction (`sea_orm::ConnectOptions`) has no `after_connect`/
//! `after_release` hook, so the RLS session-state lifecycle this deployment depends on
//! (`SET ROLE` per physical connection, `set_config(...)` per request, reset on release)
//! is built here as a standalone `sqlx::PgPool` and stored in `AppContext::shared_store`
//! (see `Hooks::after_context` in `src/app.rs`). Most of the actual request-handling query
//! path runs through this pool, not Loco's `ctx.db`; see
//! <https://github.com/yotsunagi/yorishiro/issues/221> for why.
use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
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
}

impl TenantDb {
    /// Wraps a raw pool as-is.
    /// Callers must separately guarantee that `app.current_tenant`/`app.current_workspace`
    /// get reset when a connection returns to the pool (use `connect` for production).
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Builds the production pool.
    /// The `after_connect` hook issues `SET ROLE` once per physical connection so all
    /// subsequent queries run as the `yorishiro_app` role, which cannot bypass RLS.
    /// The `after_release` hook resets `app.current_tenant`/`app.current_workspace` before
    /// returning a connection to the pool, preventing one workspace's session state from
    /// leaking to whichever workspace borrows the connection next.
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
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
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
/// call to forget.
pub async fn lock_for_update(conn: &mut sqlx::PgConnection, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(conn)
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
