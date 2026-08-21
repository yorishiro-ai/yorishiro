use async_trait::async_trait;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// What a `models/` function needs from its connection to build and run a query without naming an engine.
///
/// Keyed on the connection type rather than `sqlx::Database`, so a call site passing a concrete `&mut PgConnection` (or `&mut *tx` for a `Transaction`) has `C` inferred directly from the argument and never turbofishes.
/// `DB::Connection` is not injective, so keying on `Database` instead would break that inference at every call site.
pub trait Engine: sqlx::Connection {
    type Db: sqlx::Database<Connection = Self>;
    type Builder: sea_query::QueryBuilder + Default;

    fn builder() -> Self::Builder {
        Self::Builder::default()
    }

    /// `rows_affected` is inherent on `PgQueryResult`/`SqliteQueryResult`, not a method `sqlx::Database::QueryResult` guarantees, so a generic caller cannot reach it through a trait bound.
    fn rows_affected(result: <Self::Db as sqlx::Database>::QueryResult) -> u64;

    /// A table reference, schema-qualified on Postgres and bare on Sqlite.
    ///
    /// Postgres has `identity`/`content` as real schemas; Sqlite has no schema concept for a single-file database, so qualifying a table there is a syntax error, not a no-op.
    /// A caller writes `.from(C::schema_table("content", Entities::Table))` in place of `.from((Alias::new("content"), Entities::Table))`; the emitted table name is otherwise identical.
    fn schema_table<T: sea_query::Iden + 'static>(
        schema: &'static str,
        table: T,
    ) -> sea_query::TableRef;

    /// A primary key value to insert explicitly, or `None` to rely on the database's own default.
    ///
    /// Postgres tables default every PK to `uuidv7()`, so every `INSERT` in `models/` omits the `Id` column; this must stay `None` there, since the app never generates ids on that engine (a recorded rule, not a stylistic choice).
    /// Sqlite has no `uuidv7()` to assign, so it needs one generated here instead, and both engines are required to produce the same id shape and time ordering.
    fn generated_id() -> Option<Uuid> {
        None
    }
}

impl Engine for sqlx::PgConnection {
    type Db = sqlx::Postgres;
    type Builder = sea_query::PostgresQueryBuilder;

    fn rows_affected(result: sqlx::postgres::PgQueryResult) -> u64 {
        result.rows_affected()
    }

    fn schema_table<T: sea_query::Iden + 'static>(
        schema: &'static str,
        table: T,
    ) -> sea_query::TableRef {
        use sea_query::IntoTableRef;
        (sea_query::Alias::new(schema), table).into_table_ref()
    }
}

#[cfg(feature = "sqlite")]
impl Engine for sqlx::SqliteConnection {
    type Db = sqlx::Sqlite;
    type Builder = sea_query::SqliteQueryBuilder;

    fn rows_affected(result: sqlx::sqlite::SqliteQueryResult) -> u64 {
        result.rows_affected()
    }

    fn schema_table<T: sea_query::Iden + 'static>(
        _schema: &'static str,
        table: T,
    ) -> sea_query::TableRef {
        use sea_query::IntoTableRef;
        table.into_table_ref()
    }

    fn generated_id() -> Option<Uuid> {
        // Shares one process-wide `SharedContextV7` counter (uuid crate internals, not this
        // engine's own), which is what makes same-millisecond ids strictly ordered rather than
        // merely random: see `generated_id_is_strictly_increasing_even_within_one_millisecond`,
        // which pins this as a dependency guarantee rather than trusting it silently.
        Some(Uuid::now_v7())
    }
}

/// Prepends the engine's generated id column/value to an `INSERT`'s columns and values, or passes both through unchanged when the engine relies on the database's own default.
///
/// `sea-query`'s `columns`/`values_panic` each overwrite rather than accumulate, so a call site cannot add the id column with a second call; the full column and value lists have to be assembled once, before either is called.
/// `id_col` is a per-site argument because every `models/` module defines its own local `Id` variant on its own `Iden` enum; there is no crate-wide id column type to name here.
pub fn with_generated_id<C: Engine, T: sea_query::Iden + 'static>(
    id_col: T,
    mut cols: Vec<sea_query::DynIden>,
    mut vals: Vec<sea_query::SimpleExpr>,
) -> (Vec<sea_query::DynIden>, Vec<sea_query::SimpleExpr>) {
    if let Some(id) = C::generated_id() {
        cols.insert(0, sea_query::IntoIden::into_iden(id_col));
        vals.insert(0, id.into());
    }
    (cols, vals)
}

/// Where the deployment's data lives.
///
/// The seam between the application and its database engine sits here, at the layer that hands out connections, not around the repositories.
/// Those take `&mut PgConnection` and compose their own transactions (advisory locks inside `create`, for one), so wrapping them would mean a second implementation of every one of the 59 functions under `repositories/`, while wrapping this means four methods.
///
/// The engines differ in more than dialect, and the difference this trait deliberately does **not** hide is isolation: [`Self::acquire_for_workspace`] sets the session variables that row-level security reads, and an engine without RLS cannot honour that by setting a variable.
/// Such an engine is limited to one tenant per deployment rather than pretending, because a filter written in application code is one a single missed query silently defeats.
#[async_trait]
pub trait Storage: Send + Sync {
    /// The engine this storage runs on, and the connection type `models/` functions bound on `Engine` need.
    type Db: sqlx::Database<Connection: Engine>;

    /// A connection scoped to `tenant_id`/`workspace_id`, such that row-level security (or whatever the engine offers in its place) confines it to that workspace's rows.
    async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<Self::Db>, sqlx::Error>;

    /// The underlying pool, for the control-plane paths that connect as the migration role and so must not be scoped: signup, setup, the admin CLI.
    fn pool(&self) -> &sqlx::Pool<Self::Db>;
}

#[async_trait]
impl Storage for TenantDb {
    type Db = sqlx::Postgres;

    async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<Self::Db>, sqlx::Error> {
        TenantDb::acquire_for_workspace(self, tenant_id, workspace_id).await
    }

    fn pool(&self) -> &sqlx::Pool<Self::Db> {
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

/// Where a single-tenant deployment's data lives.
///
/// There is no session state to set: no role to switch into and no RLS variable to scope by, since Sqlite has neither.
/// [`acquire_for_workspace`](Storage::acquire_for_workspace) hands out a plain connection instead, and the isolation `TenantDb` gets from Postgres this engine gets from `tenants::refuse_if_multiple_tenants_exist` at boot: one tenant per deployment, checked once, rather than a filter every query has to remember.
#[cfg(feature = "sqlite")]
#[derive(Clone)]
pub struct SqliteDb {
    pool: sqlx::SqlitePool,
}

#[cfg(feature = "sqlite")]
impl SqliteDb {
    /// Wraps a raw pool as-is.
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Builds the production pool.
    /// Foreign keys are off by default per connection in Sqlite, unlike Postgres where they cannot be, so this crate's migrations and every write depend on this hook running before any query does.
    /// `create_if_missing(true)`: a first boot names a `.db` file that does not exist yet, the same way a first boot against Postgres names a database the migration role can already reach, and refusing to create the file would make every Sqlite deployment's first run a manual step this crate never asks of a Postgres one.
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        use std::str::FromStr;

        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

        let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &sqlx::SqlitePool {
        &self.pool
    }
}

#[cfg(feature = "sqlite")]
#[async_trait]
impl Storage for SqliteDb {
    type Db = sqlx::Sqlite;

    /// No session variables to set: the single-tenant guard at boot is what stands in for row-level isolation here.
    async fn acquire_for_workspace(
        &self,
        _tenant_id: Uuid,
        _workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, sqlx::Error> {
        self.pool.acquire().await
    }

    fn pool(&self) -> &sqlx::SqlitePool {
        SqliteDb::pool(self)
    }
}

/// Which engine a deployment is running, holding whichever connections that engine needs.
///
/// [`super::services::auth::Authenticator`] and every other seam that must work before a workspace is known (so before `Storage::acquire_for_workspace` can scope anything) takes this instead of a bare pool, so the seam itself carries the engine choice rather than assuming Postgres.
///
/// The Postgres variant carries two pools for the same reason [`crate::AppState`] on the server side does: `identity` connects with the migration role, bypassing RLS for the control-plane tables (`identity.users`/`identity.tenant_memberships`/`identity.invites`) that have no tenant/workspace context yet to scope by.
/// Sqlite has no role to separate, so its variant carries the one pool `Storage` also uses; there is no second one to name.
#[derive(Clone)]
pub enum DbHandle {
    Postgres {
        tenant: TenantDb,
        identity: PgPool,
    },
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteDb),
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

/// A held advisory lock, released when dropped or when its connection returns to the pool.
///
/// [`lock_for_update`] ends with its transaction, which is the right scope when everything guarded runs on one connection.
/// A caller whose steps each take their own connection from the pool cannot use it: there is no shared transaction for the lock to live in, and each step would take and release its own.
///
/// This is the session-scoped form for that case.
/// The lock lives on the connection rather than a transaction, so holding the connection is what holds the lock, and the guard exists to make dropping it release the lock rather than leaving it until the connection is recycled.
pub struct SessionLock {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
    key: String,
}

impl SessionLock {
    /// Blocks until the lock is held, then keeps it until [`release`](Self::release) or drop.
    ///
    /// Same key derivation as [`lock_for_update`], so the two exclude each other on the same string.
    ///
    /// **No timeout, deliberately.** `pg_advisory_lock` waits indefinitely, and a timeout here would turn "wait your turn" into "skip the ordering check", which is the thing being protected.
    /// What makes that safe is that the guarded section is bounded database work: a caller must not hold this across an outbound network call, where a hung request would queue every later holder of the same key behind it.
    /// A holder that panics is fine either way, since dropping the guard frees the lock (`a_dropped_guard_frees_the_lock_without_release` measures that rather than trusting it).
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
    ///
    /// Exposed because holding this guard means holding one connection out of the pool: a caller that goes back to the pool for its guarded work needs two connections per holder, and enough concurrent holders then exhaust the pool rather than queueing on the lock.
    /// Work that can run here should.
    pub fn conn(&mut self) -> &mut sqlx::PgConnection {
        &mut self.conn
    }

    /// Releases the lock and reports whether that failed.
    ///
    /// Dropping does the same thing without a result to check, which is why this exists: a caller that can act on the failure should not have to learn about it from a log line.
    pub async fn release(mut self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(&self.key)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/db.rs"]
mod tests;
