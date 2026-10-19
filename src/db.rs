//! SeaORM creates both PostgreSQL pools with per-connection role, timezone,
//! and release callbacks. The tenant and identity pools stay separate so RLS
//! session state cannot cross control-plane and tenant operations.
//!
//! Requests reach the database through `TenantDb::begin_for_workspace`, whose returned `DatabaseTransaction` carries both the entity API and raw SQL the entity layer can't express (JSONB containment, pgvector search, advisory locks).
use sea_orm::sqlx::{Connection, PgPool};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr, Statement, TransactionTrait,
};
use sqlite_vec::sqlite3_vec_init;
use uuid::Uuid;

/// Common backend predicates for request and service code.
pub trait AppContextBackend {
    fn is_sqlite(&self) -> bool;
    fn is_postgres(&self) -> bool;
}

impl AppContextBackend for loco_rs::app::AppContext {
    fn is_sqlite(&self) -> bool {
        self.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite
    }

    fn is_postgres(&self) -> bool {
        self.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres
    }
}

/// Registers SQLite extensions (sqlite-vec) so every connection opened on a SQLite URL
/// auto-loads them.
///
/// Called from two places — `src/bin/main.rs` (covers all CLI subcommands) and
/// `App::boot` (covers the test harness, which never runs `main.rs`) — each guarded by
/// `std::sync::Once::call_once` so the C-level registration is atomic and idempotent.
///
/// The registration must happen **before** any SQLite connection opens: `loco-rs 1.2.0`'s
/// `cli::main` calls `create_context::<H>` unconditionally at line 777, before the
/// `match cli.command` that dispatches to `Start`/`create_app`/`H::boot`.  Every
/// subcommand (`task`, `db`, `scheduler`) opens `ctx.db` there, before any `Hooks` method
/// runs.
/// Builds tenant and identity pools on PostgreSQL and stores them as the [`DbHandle`] every authenticated path resolves through.
///
/// The identity pool connects as the migration role for control-plane access (signup, setup, the admin CLI).
/// Both pools install the configured PostgreSQL session timezone per connection and reset it on release.
pub(crate) async fn install_pools(ctx: &loco_rs::app::AppContext) -> loco_rs::Result<()> {
    let database_url = &ctx.config.database.uri;
    let max_connections = ctx.config.database.max_connections;
    let timezone = crate::time::configured(ctx).to_string();
    let tenant = TenantDb::connect(database_url, max_connections, timezone.clone())
        .await
        .map_err(|e| loco_rs::Error::Message(format!("failed to build tenant pool: {e}")))?;
    let identity = connect_postgres_pool(
        database_url,
        max_connections,
        timezone.clone(),
        false,
        false,
        Some(ctx.config.database.min_connections),
        Some(std::time::Duration::from_millis(
            ctx.config.database.idle_timeout,
        )),
        ctx.config
            .database
            .acquire_timeout
            .map(std::time::Duration::from_millis),
    )
    .await
    .map_err(|e| loco_rs::Error::Message(format!("failed to build identity pool: {e}")))?;
    let identity = identity.get_postgres_connection_pool().clone();
    ctx.shared_store.insert(DbHandle { tenant, identity });
    Ok(())
}

/// Replaces Loco's PostgreSQL application pool with one that initializes the
/// configured session timezone on every physical connection.
///
/// # Errors
/// Returns an error if the configured application pool cannot be opened.
pub(crate) async fn replace_postgres_app_pool(
    ctx: &mut loco_rs::app::AppContext,
) -> loco_rs::Result<()> {
    if !ctx.is_postgres() {
        return Ok(());
    }

    let config = &ctx.config.database;
    let mut options = sea_orm::ConnectOptions::new(config.uri.clone());
    options
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(std::time::Duration::from_millis(config.connect_timeout))
        .idle_timeout(std::time::Duration::from_millis(config.idle_timeout))
        .sqlx_logging(config.enable_logging);
    if let Some(acquire_timeout) = config.acquire_timeout {
        options.acquire_timeout(std::time::Duration::from_millis(acquire_timeout));
    }

    postgres_session_options(
        &mut options,
        crate::time::configured(ctx).to_string(),
        false,
    );

    let replacement = sea_orm::Database::connect(options).await.map_err(|error| {
        loco_rs::Error::Message(format!("failed to build application pool: {error}"))
    })?;
    let previous = std::mem::replace(&mut ctx.db, replacement);
    previous.close_by_ref().await.map_err(|error| {
        loco_rs::Error::Message(format!(
            "failed to close Loco's default application pool: {error}"
        ))
    })?;
    Ok(())
}

/// Replaces Loco's SQLite application pool with one whose timestamp default uses the configured zone.
///
/// SQLite's special keyword is shadowed by a per-connection scalar so each app context owns its zone.
///
/// # Errors
/// Returns an error when a replacement connection or per-connection callback fails.
pub(crate) async fn replace_sqlite_app_pool(
    ctx: &mut loco_rs::app::AppContext,
) -> loco_rs::Result<()> {
    if !ctx.is_sqlite() {
        return Ok(());
    }
    let zone = crate::time::configured(ctx);
    let config = &ctx.config.database;
    let mut options = sea_orm::ConnectOptions::new(config.uri.clone());
    options
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(std::time::Duration::from_millis(config.connect_timeout))
        .idle_timeout(std::time::Duration::from_millis(config.idle_timeout))
        .sqlx_logging(config.enable_logging);
    let run_on_start = config.run_on_start.clone();
    if let Some(timeout) = config.acquire_timeout {
        options.acquire_timeout(std::time::Duration::from_millis(timeout));
    }
    ctx.db.close_by_ref().await.map_err(|error| {
        loco_rs::Error::Message(format!(
            "failed to close Loco's default SQLite app pool: {error}"
        ))
    })?;
    options.map_sqlx_sqlite_pool_opts(move |pool| {
        let run_on_start = run_on_start.clone();
        pool.after_connect(move |conn, _meta| {
            let run_on_start = run_on_start.clone();
            Box::pin(async move {
                register_sqlite_timestamp(conn, zone).await?;
                if let Some(sql) = run_on_start.as_deref() {
                    sea_orm::sqlx::raw_sql(sea_orm::sqlx::AssertSqlSafe(sql))
                        .execute(&mut *conn)
                        .await?;
                }
                Ok(())
            })
        })
    });
    let replacement = sea_orm::Database::connect(options).await.map_err(|error| {
        loco_rs::Error::Message(format!("failed to build SQLite app pool: {error}"))
    })?;
    ctx.db = replacement;
    Ok(())
}

async fn register_sqlite_timestamp(
    conn: &mut sea_orm::sqlx::SqliteConnection,
    timezone: chrono_tz::Tz,
) -> Result<(), sea_orm::sqlx::Error> {
    let mut handle = conn.lock_handle().await?;
    // SQLite owns this allocation after the call and invokes xDestroy on success or failure.
    let timezone = Box::into_raw(Box::new(timezone)).cast();
    // SAFETY: SQLx's locked handle excludes worker-thread SQLite calls; static C strings and valid
    // callbacks are supplied, and SQLite owns `timezone` through xDestroy after invocation.
    let result = unsafe {
        libsqlite3_sys::sqlite3_create_function_v2(
            handle.as_raw_handle().as_ptr(),
            c"current_timestamp".as_ptr(),
            0,
            libsqlite3_sys::SQLITE_UTF8,
            timezone,
            Some(sqlite_current_timestamp),
            None,
            None,
            Some(drop_sqlite_timezone),
        )
    };
    if result == libsqlite3_sys::SQLITE_OK {
        Ok(())
    } else {
        Err(sea_orm::sqlx::Error::Protocol(format!(
            "failed to register SQLite CURRENT_TIMESTAMP: {result}"
        )))
    }
}

unsafe extern "C" fn drop_sqlite_timezone(value: *mut std::ffi::c_void) {
    // SAFETY: SQLite calls xDestroy exactly once with the Box pointer transferred at registration.
    unsafe { drop(Box::from_raw(value.cast::<chrono_tz::Tz>())) };
}

unsafe extern "C" fn sqlite_current_timestamp(
    context: *mut libsqlite3_sys::sqlite3_context,
    _argument_count: std::ffi::c_int,
    _arguments: *mut *mut libsqlite3_sys::sqlite3_value,
) {
    // SAFETY: SQLite provides a live function context whose user data is the owned timezone
    // allocation. The pointer is read only and never retained beyond this callback.
    let result = std::panic::catch_unwind(|| {
        // SAFETY: the context is live for this callback, and its user data was installed above.
        let timezone = unsafe {
            libsqlite3_sys::sqlite3_user_data(context)
                .cast::<chrono_tz::Tz>()
                .as_ref()
        }
        .copied()
        .unwrap_or(chrono_tz::UTC);
        crate::time::sqlite_timestamp(chrono::Utc::now(), timezone)
    });
    match result {
        // SAFETY: `timestamp` remains live for the call and SQLITE_TRANSIENT copies it immediately.
        Ok(timestamp) => unsafe {
            libsqlite3_sys::sqlite3_result_text(
                context,
                timestamp.as_ptr().cast(),
                timestamp.len() as std::ffi::c_int,
                libsqlite3_sys::SQLITE_TRANSIENT(),
            );
        },
        // SAFETY: the static error string is NUL-terminated and remains valid for the call.
        Err(_) => unsafe {
            libsqlite3_sys::sqlite3_result_error(
                context,
                c"timestamp conversion failed".as_ptr(),
                -1,
            );
        },
    }
}

#[allow(clippy::too_many_arguments)]
/// Builds a PostgreSQL pool through SeaORM with the application's session hooks.
///
/// # Errors
/// Returns a database error if SeaORM cannot initialize the configured connection.
pub fn connect_postgres_pool(
    database_url: &str,
    max_connections: u32,
    timezone: String,
    tenant: bool,
    lazy: bool,
    min_connections: Option<u32>,
    idle_timeout: Option<std::time::Duration>,
    acquire_timeout: Option<std::time::Duration>,
) -> impl std::future::Future<Output = Result<DatabaseConnection, DbErr>> + Send + '_ {
    let mut options = sea_orm::ConnectOptions::new(database_url.to_owned());
    options.max_connections(max_connections);
    if let Some(min_connections) = min_connections {
        options.min_connections(min_connections);
    }
    if let Some(idle_timeout) = idle_timeout {
        options.idle_timeout(Some(idle_timeout));
    }
    if let Some(acquire_timeout) = acquire_timeout {
        options.acquire_timeout(acquire_timeout);
    }
    options.connect_lazy(lazy);
    postgres_session_options(&mut options, timezone, tenant);
    async move { sea_orm::Database::connect(options).await }
}

/// Installs session ownership and reset policy for every application-owned PostgreSQL pool.
fn postgres_session_options(options: &mut sea_orm::ConnectOptions, timezone: String, tenant: bool) {
    options.map_sqlx_postgres_pool_opts(move |pool| {
        let connect_timezone = timezone.clone();
        let release_timezone = timezone.clone();
        let pool = pool.after_connect(move |conn, _meta| {
            let timezone = connect_timezone.clone();
            Box::pin(async move {
                if tenant {
                    sea_orm::sqlx::query("SET ROLE yorishiro_app")
                        .execute(&mut *conn)
                        .await?;
                }
                sea_orm::sqlx::query("SELECT set_config('TimeZone', $1, false)")
                    .bind(timezone)
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        });
        pool.after_release(move |conn, _meta| {
            let timezone = release_timezone.clone();
            Box::pin(async move {
                if tenant {
                    sea_orm::sqlx::query("RESET app.current_tenant")
                        .execute(&mut *conn)
                        .await?;
                    sea_orm::sqlx::query("RESET app.current_workspace")
                        .execute(&mut *conn)
                        .await?;
                }
                sea_orm::sqlx::query("SELECT set_config('TimeZone', $1, false)")
                    .bind(timezone)
                    .execute(&mut *conn)
                    .await?;
                Ok(true)
            })
        })
    });
}

/// Reads PostgreSQL's process-level activity view for the load guard.
/// The activity view is queried with SeaQuery rather than an application entity.
pub(crate) async fn active_connections(pool: &PgPool) -> Result<i64, crate::YorishiroError> {
    use crate::error::ResultExt;

    use sea_orm::sea_query::{Alias, Asterisk, Expr, ExprTrait, Func, Query};
    let query = Query::select()
        .expr(Func::count(Expr::col(Asterisk)))
        .from(Alias::new("pg_stat_activity"))
        .and_where(Expr::col(Alias::new("datname")).eq(Func::cust(Alias::new("current_database"))))
        .and_where(Expr::col(Alias::new("state")).eq("active"))
        .to_owned();
    let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
    db.query_one_raw(db.get_database_backend().build(&query))
        .await
        .internal()?
        .ok_or_else(|| {
            crate::YorishiroError::Internal(anyhow::anyhow!("activity count returned no row"))
        })?
        .try_get_by_index(0)
        .internal()
}

/// A vector in the BLOB layout `sqlite-vec` reads: consecutive little-endian `f32` values.
///
/// Written out value by value, so the bytes are little-endian on every host rather than whatever order the host happens to use.
pub fn sqlite_vec_blob(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Public entry point for `main.rs` and `App::boot` to call.
///
/// The configured-zone timestamp function is installed separately per app-pool connection.
pub fn register_sqlite_extensions() {
    use std::mem::transmute;

    // `libsqlite3-sys` is a direct dependency (pinned to the same version that
    // `sqlx-sqlite 0.9.0` resolves) so we can call `sqlite3_auto_extension`
    // without fighting an E0432 FFI mismatch.
    use libsqlite3_sys::sqlite3_auto_extension;

    // The target type is `sqlite3_auto_extension_callback` — a C function pointer
    // that clippy's transmute-checker wants spelled out literally; it is a foreign
    // type with opaque internals so we suppress the lint instead of duplicating
    // libsqlite3-sys's bindgen output.
    static REGISTER: std::sync::Once = std::sync::Once::new();
    #[allow(clippy::missing_transmute_annotations)]
    REGISTER.call_once(|| unsafe {
        // `sqlite3_vec_init` is declared in `sqlite-vec` as a bare C function
        // pointer; `sqlite3_auto_extension` expects a function pointer matching
        // `void(*)(sqlite3*, char**, const sqlite3_api_routines*)`, which is the
        // same signature (cast to *const () is safe because both are FFI-safe).
        sqlite3_auto_extension(Some(transmute::<*const (), _>(
            sqlite3_vec_init as *const (),
        )));
    });
}

#[derive(Clone)]
pub struct TenantDb {
    pool: PgPool,
    orm: DatabaseConnection,
}

impl TenantDb {
    /// Builds the production pool.
    /// `after_connect` issues `SET ROLE` once per physical connection, so every query runs as `yorishiro_app`, which cannot bypass RLS; a failure to assume that role fails the connection rather than falling back to the connecting role.
    /// `after_release` resets the GUCs before a connection returns to the pool, covering `acquire_for_workspace`'s use outside any transaction.
    ///
    /// `connect_lazy`, not eager connect: `Hooks::after_context` runs before migrations create `yorishiro_app`.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn connect(
        database_url: &str,
        max_connections: u32,
        timezone: String,
    ) -> Result<Self, sea_orm::sqlx::Error> {
        let connection = connect_postgres_pool(
            database_url,
            max_connections,
            timezone,
            true,
            true,
            None,
            None,
            None,
        )
        .await
        .map_err(|error| sea_orm::sqlx::Error::Protocol(error.to_string()))?;
        let pool = connection.get_postgres_connection_pool().clone();
        let orm = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        Ok(TenantDb { pool, orm })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Begins a transaction scoped to `tenant_id`/`workspace_id`: the unit of work for one RLS-scoped request.
    /// `app.current_tenant`/`app.current_workspace` are set transaction-locally (`set_config(..., true)`), so Postgres RLS policies see them for every statement run on the returned transaction, entity API and raw SQL alike, and they disappear automatically at commit or rollback.
    ///
    /// The caller owns the transaction's lifetime: a write handler must call `txn.commit().await` explicitly, or every write in it is silently discarded when the transaction drops.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn begin_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<DatabaseTransaction, DbErr> {
        let txn = self.orm.begin().await?;

        // `set_config` sets PostgreSQL session variables for RLS scoping.
        // SeaORM has no API for executing non-SELECT configuration statements, so raw SQL is required.
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
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sea_orm::sqlx::pool::PoolConnection<sea_orm::sqlx::Postgres>, sea_orm::sqlx::Error>
    {
        let mut conn = self.pool.acquire().await?;

        sea_orm::sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
            .bind(tenant_id.to_string())
            .execute(conn.as_mut())
            .await?;
        sea_orm::sqlx::query("SELECT set_config('app.current_workspace', $1, false)")
            .bind(workspace_id.to_string())
            .execute(conn.as_mut())
            .await?;
        Ok(conn)
    }
}

/// Sets a PostgreSQL session's timezone using an escaped IANA name.
///
/// # Errors
/// Returns an error if the statement fails.
pub async fn set_session_timezone(db: &DatabaseConnection, timezone: &str) -> Result<(), DbErr> {
    db.execute_unprepared(&format!("SET TIME ZONE '{}'", timezone.replace('\'', "''")))
        .await?;
    Ok(())
}

/// Begins the transaction one workspace's request work runs in.
///
/// PostgreSQL scopes it to the tenant and workspace through the tenant pool, so row-level security applies.
/// SQLite has no row-level security and one tenant, so the transaction runs on the application connection.
///
/// # Errors
/// Returns an error if the transaction cannot be started, or if a PostgreSQL deployment has no tenant pool.
pub(crate) async fn begin_workspace(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<DatabaseTransaction, crate::YorishiroError> {
    use crate::error::ResultExt;

    if ctx.is_sqlite() {
        return ctx.db.begin().await.internal();
    }
    let handle = ctx
        .shared_store
        .get::<DbHandle>()
        .ok_or_else(|| crate::YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    handle
        .tenant
        .begin_for_workspace(tenant_id, workspace_id)
        .await
        .internal()
}

/// Begins a read-modify-write transaction, acquiring SQLite's writer reservation before reads.
/// Other backends retain their normal transaction semantics.
pub(crate) async fn begin_write(db: &DatabaseConnection) -> Result<DatabaseTransaction, DbErr> {
    db.begin_with_options(sea_orm::TransactionOptions {
        sqlite_transaction_mode: Some(sea_orm::SqliteTransactionMode::Immediate),
        ..Default::default()
    })
    .await
}

/// Opens a workspace-scoped write transaction without exposing backend selection to workers.
pub(crate) async fn begin_workspace_write(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<DatabaseTransaction, crate::YorishiroError> {
    use crate::error::ResultExt;

    if ctx.is_sqlite() {
        begin_write(&ctx.db).await.internal()
    } else {
        begin_workspace(ctx, tenant_id, workspace_id).await
    }
}

/// Which pools this deployment holds for control-plane vs. tenant-scoped access.
///
/// `identity` connects with the migration role, bypassing RLS for the control-plane tables (`user_users`/`tenant_memberships`/`workspace_invites`) that have no tenant/workspace context yet to scope by.
#[derive(Clone)]
pub struct DbHandle {
    pub tenant: TenantDb,
    pub identity: PgPool,
}

impl DbHandle {
    pub(crate) fn identity_connection(&self) -> DatabaseConnection {
        sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(self.identity.clone())
    }
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
/// `insert` is a parameter because an insert normally takes the column's database default. `schema_schemas` is the exception and passes `false` on both paths, since SQLite refuses a non-constant default on a column added to an existing table.
///
/// There is deliberately no counterpart for `created_at`: that column has `NOT NULL DEFAULT now()` on both backends, so nothing in application code should be able to move it.
pub fn stamped_updated_at(
    insert: bool,
    current: sea_orm::ActiveValue<chrono::DateTime<chrono::FixedOffset>>,
) -> sea_orm::ActiveValue<chrono::DateTime<chrono::FixedOffset>> {
    if !insert && !current.is_set() {
        sea_orm::ActiveValue::Set(crate::time::now().into())
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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
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
/// A no-op on SQLite, which has no named-lock primitive to substitute: it excludes nothing there, so a caller that needs mutual exclusion on SQLite must get it from a conditional write of its own.
///
/// That is sound rather than merely convenient, because every caller here locks, reads a count or existence check, then writes within the same transaction gated on that read.
/// SQLite allows one write transaction at a time, so a transaction committing after another has written gets `SQLITE_BUSY` and fails whole; the TOCTOU this lock closes on Postgres surfaces as a retryable error rather than a silently-accepted inconsistent write.
/// A caller holding the lock for some other reason would need its own justification, and none does.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
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

/// Takes the transaction-scoped advisory lock `key` if no other transaction holds it, without waiting.
///
/// The lock lasts until `conn`'s transaction ends.
/// Always `true` on SQLite, which has no named-lock primitive and one writer process, so there is no other holder to exclude.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn try_lock_for_update(conn: &impl ConnectionTrait, key: &str) -> Result<bool, DbErr> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return Ok(true);
    }
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0)) AS locked",
            [key.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("advisory lock query returned no row".into()))?;
    row.try_get("", "locked")
}

/// A guard that holds a session-scoped advisory lock on a workspace for reindex serialization.
///
/// `conn` is detached so returning it to the pool does not release the lock.
/// `release().await` closes the connection asynchronously, sends PostgreSQL the `Terminate`
/// message, ends the session, and releases the lock.
///
/// `Drop` is only a panic-unwind backstop: it tries to spawn `conn.close()` on the current
/// runtime, which may or may not succeed depending on the runtime state, but is better
/// than leaving the connection open with no cleanup attempt at all.
pub struct WorkspaceReindexLockGuard {
    conn: Option<sea_orm::sqlx::PgConnection>,
}

impl WorkspaceReindexLockGuard {
    /// Closes the detached connection and releases the advisory lock.
    ///
    /// Returns `Ok(())` on success. If the connection was already released
    /// (double-`release` is a no-op), returns `Ok(())` without error.
    /// Also logs any close failure for diagnostics.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn release(mut self) -> Result<(), sea_orm::sqlx::Error> {
        if let Some(conn) = self.conn.take() {
            let result = conn.close().await;
            if let Err(ref e) = result {
                tracing::error!(error = %e, "failed to close reindex lock connection");
            }
            result
        } else {
            Ok(())
        }
    }
}

impl Drop for WorkspaceReindexLockGuard {
    fn drop(&mut self) {
        // This path runs only if the caller forgot to call `release()` — a
        // logic error.  Try to close the connection asynchronously; the
        // explicit `release().await` is the intended path.
        if let Some(conn) = self.conn.take()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                let _ = conn.close().await;
            });
        }
        // If we cannot get a runtime handle (no current runtime, or a
        // different runtime), we have no way to close the connection
        // asynchronously. The client socket closes when the process exits.
    }
}

/// Acquires a session-scoped advisory lock on a workspace for reindex serialization.
///
/// Acquires `pg_advisory_lock` on a bare pooled connection and detaches it. The lock is held
/// until `WorkspaceReindexLockGuard::release()` is called, which closes the connection and
/// ends the session. Pool return keeps the session alive, so the lock would persist and cause
/// the next reindex to re-enter (advisory locks are per-session, not per-connection).
///
/// Does not time out: a legitimate reindex over a large workspace can block for a long time,
/// and imposing a bound would risk killing a run that is just busy. The `tracing::info!`
/// before and after acquisition makes a wait visible rather than a silent hang.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn acquire_workspace_reindex_lock(
    pool: PgPool,
    workspace_id: Uuid,
) -> Result<WorkspaceReindexLockGuard, sea_orm::sqlx::Error> {
    let mut conn = pool.acquire().await?;

    let key = workspace_id.to_string();
    tracing::info!(%key, "waiting for workspace reindex lock");

    sea_orm::sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(&key)
        .execute(conn.as_mut())
        .await?;

    tracing::info!(%key, "acquired workspace reindex lock");

    // Detach the connection: returning it to the pool would keep the session (and the lock)
    // alive. `release()` on the guard closes the connection, ending the session.
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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn reindex_workspace_with_lock(
    pool: PgPool,
    workspace_id: Uuid,
    conn: &impl ConnectionTrait,
    candidate_ids: &[Uuid],
    provider: &dyn crate::services::embedding::EmbeddingProvider,
) -> Result<crate::models::entity_embeddings::ReindexOutcome, crate::YorishiroError> {
    let lock = acquire_workspace_reindex_lock(pool, workspace_id)
        .await
        .map_err(|err| {
            crate::YorishiroError::Internal(anyhow::anyhow!(
                "failed to acquire workspace lock: {err}"
            ))
        })?;
    let outcome = crate::models::entity_embeddings::reindex_workspace(
        conn,
        workspace_id,
        candidate_ids,
        provider,
    )
    .await;
    let _ = lock.release().await;
    match outcome {
        Ok(ok) => Ok(ok),
        Err(err) => Err(err),
    }
}
