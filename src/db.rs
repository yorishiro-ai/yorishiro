//! Separate identity and tenant pools isolate PostgreSQL session state.
//!
//! `SeaORM` installs per-connection role, timezone, and release callbacks.
//! RLS session state cannot cross control-plane and tenant operations.
//!
//! Requests reach the database through `TenantDb::begin_for_workspace`, whose returned `DatabaseTransaction` carries both the entity API and raw SQL the entity layer can't express (JSONB containment, pgvector search, advisory locks).
use sea_orm::sqlx::PgPool;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr, Statement, TransactionTrait,
};
use sqlite_vec::sqlite3_vec_init;
use uuid::Uuid;

/// Which backend a connection talks to.
/// Every backend decision outside this module asks through these, so there is one place that names the backends.
pub(crate) fn is_sqlite(conn: &impl ConnectionTrait) -> bool {
    conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite
}

pub(crate) fn is_postgres(conn: &impl ConnectionTrait) -> bool {
    conn.get_database_backend() == sea_orm::DatabaseBackend::Postgres
}

/// The control-plane transaction an IAM check runs in.
///
/// PostgreSQL hides the IAM tables from the tenant pool, so the check runs in its own write transaction on the identity pool, which the caller keeps open until the workspace mutation commits.
/// SQLite has one pool and no row-level security: the caller's own transaction is the one to check in, so there is none to open.
///
/// # Errors
/// Returns an error if the identity transaction cannot be opened.
pub(crate) async fn begin_authorization(
    db: &DatabaseConnection,
) -> Result<Option<DatabaseTransaction>, DbErr> {
    if is_postgres(db) {
        Ok(Some(begin_write(db).await?))
    } else {
        Ok(None)
    }
}

fn connection_options(config: &loco_rs::config::Database) -> sea_orm::ConnectOptions {
    let mut options = sea_orm::ConnectOptions::new(&config.uri);
    options
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(std::time::Duration::from_millis(config.connect_timeout))
        .idle_timeout(std::time::Duration::from_millis(config.idle_timeout))
        .sqlx_logging(config.enable_logging);
    if let Some(timeout) = config.acquire_timeout {
        options.acquire_timeout(std::time::Duration::from_millis(timeout));
    }
    options
}

/// Builds the tenant pool and stores it with the existing identity pool in [`DbHandle`].
///
/// The identity pool connects as the migration role for control-plane access (signup, setup, the admin CLI).
/// Both pools install the configured PostgreSQL session timezone per connection and reset it on release.
pub(crate) async fn install_pools(ctx: &loco_rs::app::AppContext) -> loco_rs::Result<()> {
    let timezone = crate::data::time::configured(ctx).to_string();
    let tenant = TenantDb::connect(&ctx.config.database, timezone)
        .await
        .map_err(|e| loco_rs::Error::Message(format!("failed to build tenant pool: {e}")))?;
    let identity = ctx.db.get_postgres_connection_pool().clone();
    ctx.shared_store.insert(DbHandle { tenant, identity });
    Ok(())
}

/// Replaces Loco's application pool with the backend's configured timezone hooks.
///
/// # Errors
/// Returns an error if the configured application pool cannot be opened.
pub(crate) async fn replace_app_pool(ctx: &mut loco_rs::app::AppContext) -> loco_rs::Result<()> {
    let config = &ctx.config.database;
    let mut options = connection_options(config);
    let zone = crate::data::time::configured(ctx);
    if is_sqlite(&ctx.db) {
        require_min_sqlite_connections(config.max_connections).map_err(loco_rs::Error::Message)?;
        // Keep Loco's per-connection PRAGMAs and its busy_timeout-aware acquire budget.
        let loco_pool = ctx.db.get_sqlite_connection_pool();
        let loco_options = (*loco_pool.connect_options()).clone();
        options.acquire_timeout(loco_pool.options().get_acquire_timeout());
        // Close before reopening SQLite so connection-local functions own the configured zone.
        ctx.db.close_by_ref().await.map_err(|error| {
            loco_rs::Error::Message(format!(
                "failed to close Loco's default SQLite app pool: {error}"
            ))
        })?;
        options.map_sqlx_sqlite_opts(move |_| loco_options.clone());
        options.map_sqlx_sqlite_pool_opts(move |pool| {
            pool.after_connect(move |conn, _meta| {
                Box::pin(async move {
                    register_sqlite_timestamp(conn, zone).await?;
                    Ok(())
                })
            })
        });
    } else if is_postgres(&ctx.db) {
        postgres_session_options(&mut options, zone.to_string(), PoolRole::Identity);
    } else {
        return Ok(());
    }
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
    let result = std::panic::catch_unwind(|| {
        // SAFETY: the context is live for this callback, and its user data was installed above.
        let timezone = unsafe {
            libsqlite3_sys::sqlite3_user_data(context)
                .cast::<chrono_tz::Tz>()
                .as_ref()
        }
        .copied()
        .unwrap_or(chrono_tz::UTC);
        let timestamp = crate::data::time::sqlite_timestamp(&crate::data::time::now(), timezone);
        std::ffi::c_int::try_from(timestamp.len()).map(|length| (timestamp, length))
    });
    match result {
        // SAFETY: `timestamp` remains live for the call and SQLITE_TRANSIENT copies it immediately.
        Ok(Ok((timestamp, length))) => unsafe {
            libsqlite3_sys::sqlite3_result_text(
                context,
                timestamp.as_ptr().cast(),
                length,
                libsqlite3_sys::SQLITE_TRANSIENT(),
            );
        },
        // SAFETY: the static error string is NUL-terminated and remains valid for the call.
        Ok(Err(_)) | Err(_) => unsafe {
            libsqlite3_sys::sqlite3_result_error(
                context,
                c"timestamp conversion failed".as_ptr(),
                -1,
            );
        },
    }
}

#[derive(Clone, Copy)]
enum PoolRole {
    Identity,
    Tenant,
}

impl PoolRole {
    async fn initialize(
        self,
        conn: &mut sea_orm::sqlx::PgConnection,
    ) -> Result<(), sea_orm::sqlx::Error> {
        if matches!(self, Self::Tenant) {
            sea_orm::sqlx::query("SET ROLE yorishiro_app")
                .execute(conn)
                .await?;
        }
        Ok(())
    }

    async fn reset(
        self,
        conn: &mut sea_orm::sqlx::PgConnection,
    ) -> Result<(), sea_orm::sqlx::Error> {
        if matches!(self, Self::Tenant) {
            sea_orm::sqlx::raw_sql("RESET app.current_tenant; RESET app.current_workspace")
                .execute(conn)
                .await?;
        }
        Ok(())
    }
}

/// Installs session ownership and reset policy for every application-owned PostgreSQL pool.
fn postgres_session_options(
    options: &mut sea_orm::ConnectOptions,
    timezone: String,
    role: PoolRole,
) {
    options.map_sqlx_postgres_pool_opts(move |pool| {
        let connect_timezone = timezone.clone();
        let release_timezone = timezone.clone();
        let pool = pool.after_connect(move |conn, _meta| {
            let timezone = connect_timezone.clone();
            Box::pin(async move {
                role.initialize(conn).await?;
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
                role.reset(conn).await?;
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
/// The activity view is queried with `SeaQuery` rather than an application entity.
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
        .ok_or_else(|| crate::YorishiroError::Internal("activity count returned no row".into()))?
        .try_get_by_index(0)
        .internal()
}

/// A vector in the BLOB layout `sqlite-vec` reads: consecutive little-endian `f32` values.
///
/// Written out value by value, so the bytes are little-endian on every host rather than whatever order the host happens to use.
#[must_use]
pub fn sqlite_vec_blob(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Registers sqlite-vec once before `main.rs` or `App::boot` opens any SQLite connections.
/// The CLI creates its context before dispatching subcommands, so registration cannot wait for a hook.
///
/// The configured-zone timestamp function is installed separately per app-pool connection.
pub fn register_sqlite_extensions() {
    use std::mem::transmute;

    // A direct dependency on SQLx's SQLite library keeps extension registration on the same engine as application connections.
    use libsqlite3_sys::sqlite3_auto_extension;

    // The target type is `sqlite3_auto_extension_callback` — a C function pointer
    // that clippy's transmute-checker wants spelled out literally; it is a foreign
    // type with opaque internals so we suppress the lint instead of duplicating
    // libsqlite3-sys's bindgen output.
    static REGISTER: std::sync::Once = std::sync::Once::new();
    #[expect(
        clippy::missing_transmute_annotations,
        reason = "SQLite registers the destructor through a C function pointer type"
    )]
    // SAFETY: sqlite-vec exports the SQLite extension entry-point ABI, and Once registers it before any application connection opens.
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
    pub(crate) async fn connect(
        config: &loco_rs::config::Database,
        timezone: String,
    ) -> Result<Self, sea_orm::sqlx::Error> {
        let mut options = connection_options(config);
        options.connect_lazy(true);
        postgres_session_options(&mut options, timezone, PoolRole::Tenant);
        let connection = sea_orm::Database::connect(options)
            .await
            .map_err(|error| sea_orm::sqlx::Error::Protocol(error.to_string()))?;
        let pool = connection.get_postgres_connection_pool().clone();
        let orm = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        Ok(Self { pool, orm })
    }

    #[must_use]
    pub const fn pool(&self) -> &PgPool {
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
            "SELECT set_config('app.current_tenant', $1, true), set_config('app.current_workspace', $2, true)",
            [tenant_id.to_string().into(), workspace_id.to_string().into()],
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

        sea_orm::sqlx::query(
            "SELECT set_config('app.current_tenant', $1, false), set_config('app.current_workspace', $2, false)",
        )
            .bind(tenant_id.to_string())
            .bind(workspace_id.to_string())
            .execute(conn.as_mut())
            .await?;
        Ok(conn)
    }

    pub(crate) async fn acquire_for_tenant(
        &self,
        tenant_id: Uuid,
    ) -> Result<sea_orm::sqlx::pool::PoolConnection<sea_orm::sqlx::Postgres>, sea_orm::sqlx::Error>
    {
        let mut conn = self.pool.acquire().await?;
        sea_orm::sqlx::query(
            "SELECT set_config('app.current_tenant', $1, false), set_config('app.current_workspace', '', false)",
        )
        .bind(tenant_id.to_string())
        .execute(conn.as_mut())
        .await?;
        Ok(conn)
    }
}

/// Begins the transaction one workspace's request work runs in.
///
/// PostgreSQL scopes it to the tenant and workspace through the tenant pool, so row-level security applies.
/// SQLite has no row-level security and one tenant, so the transaction runs on the application connection.
///
/// # Errors
/// Returns an error if the transaction cannot be started, or if a PostgreSQL deployment has no tenant pool.
pub async fn begin_workspace(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<DatabaseTransaction, crate::YorishiroError> {
    use crate::error::ResultExt;

    if is_sqlite(&ctx.db) {
        return ctx.db.begin().await.internal();
    }
    let handle = ctx
        .shared_store
        .get::<DbHandle>()
        .ok_or_else(|| crate::YorishiroError::Internal("DbHandle missing".into()))?;
    handle
        .tenant
        .begin_for_workspace(tenant_id, workspace_id)
        .await
        .internal()
}

/// Opens a read-only PostgreSQL transaction for an already authorized set of workspaces.
pub(crate) async fn begin_workspace_list(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    workspaces: &[Uuid],
) -> Result<DatabaseTransaction, crate::YorishiroError> {
    use crate::error::ResultExt;

    if is_sqlite(&ctx.db) {
        return ctx.db.begin().await.internal();
    }
    let handle = ctx
        .shared_store
        .get::<DbHandle>()
        .ok_or_else(|| crate::YorishiroError::Internal("DbHandle missing".into()))?;
    let txn = handle
        .tenant
        .orm
        .begin_with_config(None, Some(sea_orm::AccessMode::ReadOnly))
        .await
        .internal()?;
    txn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT set_config('app.current_tenant', $1, true), set_config('app.current_workspace', $2, true), set_config('app.workspace_list', ($3::uuid[])::text, true)",
        [
            tenant_id.to_string().into(),
            Uuid::nil().to_string().into(),
            workspaces.to_vec().into(),
        ],
    ))
    .await
    .internal()?;
    Ok(txn)
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

    if is_sqlite(&ctx.db) {
        begin_write(&ctx.db).await.internal()
    } else {
        begin_workspace(ctx, tenant_id, workspace_id).await
    }
}

/// Which pools this deployment holds for control-plane vs. tenant-scoped access.
///
/// `identity` connects with the migration role, bypassing RLS for the control-plane tables (`user_users`/`tenant_memberships`) that have no tenant/workspace context yet to scope by.
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

/// A `UUIDv7` for a primary key `before_save` hook to set on SQLite, or `ActiveValue::NotSet` to leave the column alone.
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
#[must_use]
pub fn stamped_updated_at(
    insert: bool,
    current: sea_orm::ActiveValue<chrono::DateTime<chrono::FixedOffset>>,
) -> sea_orm::ActiveValue<chrono::DateTime<chrono::FixedOffset>> {
    if !insert && !current.is_set() {
        sea_orm::ActiveValue::Set(crate::data::time::now().into())
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
