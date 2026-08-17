use async_trait::async_trait;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// The connection a request runs on, already scoped to its tenant and workspace.
pub type ScopedConnection = sqlx::pool::PoolConnection<sqlx::Postgres>;

/// Where the deployment's data lives.
///
/// The seam between the application and its database engine sits here, at the layer that hands out connections, not around the repositories.
/// Those take `&mut PgConnection` and compose their own transactions (advisory locks inside `create`, for one), so wrapping them would mean a second implementation of every one of the 59 functions under `repositories/`, while wrapping this means four methods.
///
/// The engines differ in more than dialect, and the difference this trait deliberately does **not** hide is isolation: [`Self::acquire_for_workspace`] sets the session variables that row-level security reads, and an engine without RLS cannot honour that by setting a variable.
/// Such an engine is limited to one tenant per deployment rather than pretending, because a filter written in application code is one a single missed query silently defeats.
#[async_trait]
pub trait Storage: Send + Sync {
    /// A connection scoped to `tenant_id`/`workspace_id`, such that row-level security (or whatever the engine offers in its place) confines it to that workspace's rows.
    async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<ScopedConnection, sqlx::Error>;

    /// The underlying pool, for the control-plane paths that connect as the migration role and so must not be scoped: signup, setup, the admin CLI.
    fn pool(&self) -> &PgPool;
}

#[async_trait]
impl Storage for TenantDb {
    async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<ScopedConnection, sqlx::Error> {
        TenantDb::acquire_for_workspace(self, tenant_id, workspace_id).await
    }

    fn pool(&self) -> &PgPool {
        TenantDb::pool(self)
    }
}

#[derive(Clone)]
pub struct TenantDb {
    pool: PgPool,
}

impl TenantDb {
    /// Wraps a raw pool as-is.
    /// Callers must separately guarantee that `app.current_tenant`/ `app.current_workspace` get reset when a connection returns to the pool (use `connect` for production).
    /// This also doesn't switch roles, so tenant isolation won't hold if `pool`'s connection role can bypass RLS: intended for direct use in migrations and tests.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Builds the production pool.
    /// The `after_connect` hook issues `SET ROLE` once per physical connection so all subsequent queries run as the `yorishiro_app` role, which cannot bypass RLS (the login role behind `database_url` can remain a superuser, since a superuser can `SET ROLE` to any role without needing membership).
    /// The `after_release` hook resets `app.current_tenant`/`app.current_workspace` before returning a connection to the pool, preventing one workspace's session state from leaking to whichever workspace borrows the connection next.
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // `SET ROLE` is a session/connection-control statement, not DML: sea-query only builds SELECT/INSERT/UPDATE/DELETE, so this has no query-builder form and stays raw SQL.
                    sqlx::query("SET ROLE yorishiro_app")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .after_release(|conn, _meta| {
                Box::pin(async move {
                    // `RESET` is a session-control statement (not DML): same reason as `SET ROLE` above for staying raw SQL.
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

    /// Sets the session variables `app.current_tenant` and `app.current_workspace` on this connection so RLS can isolate both the tenant-level control-plane rows and the workspace-scoped content rows.
    ///
    /// Using `is_local=false` (session-level) matters: `is_local=true` (equivalent to `SET LOCAL`) would be discarded as soon as the implicit single-statement transaction ends when called outside an explicit transaction, causing later queries to see `current_setting(...)` as an empty string, i.e. isolation breaks.
    pub async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;

        // Under `cargo test` only, and only because `sqlx::test` hands out a pool that connects as the owner.
        // Production pools take the role from `connect`'s `after_connect`; tests build theirs with `TenantDb::new`, so without this a test runs with privileges no request ever has, and a missing GRANT is invisible to it.
        //
        // Redundant in production rather than wrong there, since `SET ROLE` to the role already held is a no-op, but it costs a round trip, so it stays behind `cfg(test)`.
        #[cfg(test)]
        sqlx::query("SET ROLE yorishiro_app")
            .execute(conn.as_mut())
            .await?;
        // `set_config(...)` sets a session GUC for RLS to read via `current_setting(...)`:
        // it's a function call with no table operand, so it has no SELECT/INSERT/UPDATE/DELETE form for sea-query to build; stays raw SQL like the session commands in `connect`.
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

/// Serializes a transaction against others naming the same `key`, until it commits.
///
/// A check-then-insert needs this: two transactions can both read "under the quota" and both insert, and neither the quota nor the uniqueness rule is expressible as a constraint the database would catch.
/// The lock is transaction-scoped, so it releases on commit or rollback without an unlock call to forget.
///
/// Collected here rather than written at each call site because it is the one piece of the exclusion that is engine-specific: advisory locks are PostgreSQL's, and an engine without them needs something else: SQLite would serialize the whole database with `BEGIN IMMEDIATE`, which is coarser but sound for a deployment holding one tenant.
pub async fn lock_for_update(conn: &mut sqlx::PgConnection, key: &str) -> Result<(), sqlx::Error> {
    // `pg_advisory_xact_lock(...)` is a function call with no table operand, so sea-query has no form for it: the same category as the session commands above.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(key)
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/db.rs"]
mod tests;
