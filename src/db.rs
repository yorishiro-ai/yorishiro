//! Raw sqlx connection handling that sits beside Loco's `sea_orm::DatabaseConnection`, not through it.
//!
//! Loco's standard database bootstrap does not expose the session-state hooks
//! this deployment needs, so the RLS lifecycle is built here as a standalone
//! `sqlx::PgPool` and stored in `AppContext::shared_store` (see
//! `Hooks::after_context` in `src/app.rs`).  SeaORM itself supports
//! `ConnectOptions::after_connect`; this application needs a separate pool so
//! identity and tenant session state cannot be mixed.
//! That pool is also wrapped as a `sea_orm::DatabaseConnection`, which preserves its `after_connect` hook: the hook belongs to the sqlx pool, not to SeaORM's wrapper.
//!
//! Requests reach the database through `TenantDb::begin_for_workspace`, whose returned `DatabaseTransaction` carries both the entity API and raw SQL the entity layer can't express (JSONB containment, pgvector search, advisory locks).
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr, Statement, TransactionTrait,
};
use sqlite_vec::sqlite3_vec_init;
#[cfg(feature = "enterprise")]
use sqlx::ConnectOptions;
#[cfg(not(feature = "enterprise"))]
use sqlx::Connection;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
#[cfg(feature = "enterprise")]
use sqlx::sqlite::SqliteConnectOptions;
#[cfg(feature = "enterprise")]
use sqlx::{Connection, PgConnection};
#[cfg(feature = "enterprise")]
use std::fs::{File, OpenOptions};
#[cfg(feature = "enterprise")]
use std::path::{Path, PathBuf};
#[cfg(feature = "enterprise")]
use std::str::FromStr;
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
/// Builds the tenant and identity pools on PostgreSQL and stores them as the [`DbHandle`] every authenticated path resolves through.
///
/// The identity pool connects as the migration role for control-plane access (signup, setup, the admin CLI), so it needs no hooks: it never scopes to a workspace.
pub(crate) async fn install_pools(ctx: &loco_rs::app::AppContext) -> loco_rs::Result<()> {
    let database_url = &ctx.config.database.uri;
    let max_connections = ctx.config.database.max_connections;
    let tenant = TenantDb::connect(database_url, max_connections)
        .await
        .map_err(|e| loco_rs::Error::Message(format!("failed to build tenant pool: {e}")))?;
    let identity = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
        .map_err(|e| loco_rs::Error::Message(format!("failed to build identity pool: {e}")))?;
    ctx.shared_store.insert(DbHandle { tenant, identity });
    Ok(())
}

/// A vector in the BLOB layout `sqlite-vec` reads: consecutive little-endian `f32` values.
///
/// Written out value by value, so the bytes are little-endian on every host rather than whatever order the host happens to use.
pub(crate) fn sqlite_vec_blob(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod vector_blob_tests {
    use super::sqlite_vec_blob;

    #[test]
    fn blob_is_little_endian_f32_in_order() {
        assert_eq!(
            sqlite_vec_blob(&[1.0, -2.0]),
            [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0]
        );
        assert!(sqlite_vec_blob(&[]).is_empty());
    }
}

/// Public entry point for `main.rs` and `App::boot` to call.
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

/// Which pools this deployment holds for control-plane vs. tenant-scoped access.
///
/// `identity` connects with the migration role, bypassing RLS for the control-plane tables (`user_users`/`tenant_memberships`/`workspace_invites`) that have no tenant/workspace context yet to scope by.
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
/// `insert` is a parameter because an insert normally takes the column's database default. `schema_schemas` is the exception and passes `false` on both paths, since SQLite refuses a non-constant default on a column added to an existing table.
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

/// Ownership held for the complete scheduler task, including queue dispatch.
#[cfg(feature = "enterprise")]
pub(crate) enum SchedulerOwnership {
    Postgres {
        conn: Option<PgConnection>,
        key: String,
    },
    Sqlite {
        file: Option<File>,
        path: PathBuf,
    },
}

#[cfg(feature = "enterprise")]
impl SchedulerOwnership {
    pub(crate) fn sqlite_path(&self) -> Option<&Path> {
        match self {
            Self::Sqlite { path, .. } => Some(path),
            Self::Postgres { .. } => None,
        }
    }

    /// Explicitly releases ownership, then drops the detached session or file handle.
    pub(crate) async fn release(mut self) -> Result<(), String> {
        match &mut self {
            Self::Postgres { conn, key } => {
                let Some(mut conn) = conn.take() else {
                    return Ok(());
                };
                let unlock = sqlx::query_scalar::<_, bool>(
                    "SELECT pg_advisory_unlock(hashtextextended($1, 0))",
                )
                .bind(key.as_str())
                .fetch_one(&mut conn)
                .await;
                let close = conn.close().await;
                match unlock {
                    Err(err) => Err(format!("scheduler advisory unlock failed: {err}")),
                    Ok(false) => Err("scheduler advisory unlock returned false".into()),
                    Ok(true) => close.map_err(|err| format!("scheduler lock close failed: {err}")),
                }
            }
            Self::Sqlite { file, path } => {
                let Some(file) = file.take() else {
                    return Ok(());
                };
                unlock_sqlite_scheduler_file(&file)
                    .map_err(|err| format!("failed to unlock {}: {err}", path.display()))
            }
        }
    }
}

#[cfg(feature = "enterprise")]
impl Drop for SchedulerOwnership {
    fn drop(&mut self) {
        if let Self::Postgres { conn, .. } = self
            && let Some(conn) = conn.take()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                let _ = conn.close().await;
            });
        }
        // Closing the SQLite file handle releases its OS lock even if explicit cleanup is skipped.
    }
}

/// Acquires scheduler ownership using the effective Loco database configuration.
///
/// PostgreSQL uses a detached session-scoped advisory lock so it remains held through dispatch.
/// SQLite uses a non-blocking lock file beside the configured database file.
#[cfg(feature = "enterprise")]
pub(crate) async fn acquire_scheduler_ownership(
    ctx: &loco_rs::app::AppContext,
    key: &str,
) -> Result<Option<SchedulerOwnership>, String> {
    if ctx.is_postgres() {
        let db = ctx
            .shared_store
            .get::<DbHandle>()
            .ok_or_else(|| "scheduler ownership requires the PostgreSQL DbHandle".to_string())?;
        return acquire_postgres_scheduler_lock(db.identity.clone(), key).await;
    }
    if ctx.is_sqlite() {
        return acquire_sqlite_scheduler_lock(&ctx.config.database.uri, key);
    }
    Err("scheduler ownership is unsupported for this database backend".into())
}

#[cfg(feature = "enterprise")]
async fn acquire_postgres_scheduler_lock(
    pool: PgPool,
    key: &str,
) -> Result<Option<SchedulerOwnership>, String> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|err| format!("scheduler ownership connection: {err}"))?;
    let locked =
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
            .bind(key)
            .fetch_one(conn.as_mut())
            .await;
    let locked = match locked {
        Ok(locked) => locked,
        Err(err) => {
            let _ = conn.close().await;
            return Err(format!("scheduler ownership lock: {err}"));
        }
    };
    if !locked {
        conn.close()
            .await
            .map_err(|err| format!("scheduler contention connection close: {err}"))?;
        return Ok(None);
    }
    Ok(Some(SchedulerOwnership::Postgres {
        conn: Some(conn.detach()),
        key: key.to_string(),
    }))
}

#[cfg(feature = "enterprise")]
fn acquire_sqlite_scheduler_lock(
    uri: &str,
    _key: &str,
) -> Result<Option<SchedulerOwnership>, String> {
    let path = sqlite_scheduler_lock_path(uri)?;
    let Some(file) = try_lock_sqlite_scheduler_file(&path)? else {
        return Ok(None);
    };
    Ok(Some(SchedulerOwnership::Sqlite {
        file: Some(file),
        path,
    }))
}

#[cfg(feature = "enterprise")]
fn sqlite_scheduler_lock_path(uri: &str) -> Result<PathBuf, String> {
    let options = SqliteConnectOptions::from_str(uri)
        .map_err(|err| format!("invalid SQLite database URI for scheduler lock: {err}"))?;
    if sqlite_options_are_in_memory(&options) {
        return Err(
            "SQLite scheduler ownership requires a file database; in-memory SQLite is unsupported"
                .into(),
        );
    }
    let database = options.get_filename();
    if database.as_os_str().is_empty() {
        return Err("SQLite scheduler ownership requires a database file path".into());
    }
    let database = if database.exists() {
        std::fs::canonicalize(database)
            .map_err(|err| format!("failed to resolve SQLite database path: {err}"))?
    } else {
        let file_name = database.file_name().ok_or_else(|| {
            "SQLite scheduler database URI has no final path component".to_string()
        })?;
        let parent = database
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::canonicalize(parent)
            .map_err(|err| format!("failed to resolve SQLite database directory: {err}"))?
            .join(file_name)
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let links = std::fs::metadata(&database)
            .map(|metadata| metadata.nlink())
            .unwrap_or(1);
        if links > 1 {
            return Err(format!(
                "SQLite scheduler ownership does not support hard-linked database files: {} has {links} links",
                database.display()
            ));
        }
    }
    Ok(PathBuf::from(format!(
        "{}.scheduler.lock",
        database.display()
    )))
}

#[cfg(feature = "enterprise")]
fn sqlite_options_are_in_memory(options: &SqliteConnectOptions) -> bool {
    let filename = options.get_filename();
    if filename.as_os_str().is_empty() {
        return true;
    }
    if filename
        .to_str()
        .is_some_and(|path| path.starts_with("file:sqlx-in-memory-"))
    {
        return true;
    }
    options
        .to_url_lossy()
        .query_pairs()
        .any(|(key, value)| key == "mode" && value == "memory")
}

#[cfg(feature = "enterprise")]
fn try_lock_sqlite_scheduler_file(path: &Path) -> Result<Option<File>, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|err| format!("failed to open scheduler lock {}: {err}", path.display()))?;
    match lock_sqlite_scheduler_file(&file) {
        Ok(()) => Ok(Some(file)),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(err) => Err(format!(
            "failed to lock scheduler file {}: {err}",
            path.display()
        )),
    }
}

#[cfg(unix)]
#[cfg(feature = "enterprise")]
fn lock_sqlite_scheduler_file(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
#[cfg(feature = "enterprise")]
fn unlock_sqlite_scheduler_file(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    const LOCK_UN: i32 = 8;
    let result = unsafe { flock(file.as_raw_fd(), LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
#[cfg(feature = "enterprise")]
fn lock_sqlite_scheduler_file(_file: &File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SQLite scheduler file locking is unsupported on this platform",
    ))
}

#[cfg(not(unix))]
#[cfg(feature = "enterprise")]
fn unlock_sqlite_scheduler_file(_file: &File) -> std::io::Result<()> {
    Ok(())
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
    conn: Option<sqlx::PgConnection>,
}

impl WorkspaceReindexLockGuard {
    /// Closes the detached connection and releases the advisory lock.
    ///
    /// Returns `Ok(())` on success. If the connection was already released
    /// (double-`release` is a no-op), returns `Ok(())` without error.
    /// Also logs any close failure for diagnostics.
    pub async fn release(mut self) -> Result<(), sqlx::Error> {
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
pub async fn acquire_workspace_reindex_lock(
    pool: PgPool,
    workspace_id: Uuid,
) -> Result<WorkspaceReindexLockGuard, sqlx::Error> {
    let mut conn = pool.acquire().await?;

    let key = workspace_id.to_string();
    tracing::info!(%key, "waiting for workspace reindex lock");

    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
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

#[cfg(all(test, feature = "enterprise"))]
mod tests {
    use sqlx::Connection;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::fs::OpenOptions;
    use std::io::BufRead;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::str::FromStr;
    use std::time::Duration;
    use tempfile::tempdir;

    use super::PgPoolOptions;
    use super::{
        acquire_sqlite_scheduler_lock, lock_sqlite_scheduler_file, sqlite_scheduler_lock_path,
        unlock_sqlite_scheduler_file,
    };

    fn postgres_test_url() -> Option<String> {
        let url = std::env::var("DATABASE_URL").ok();
        let is_postgres = url
            .as_deref()
            .is_some_and(|url| url.starts_with("postgres://") || url.starts_with("postgresql://"));
        if std::env::var("LOCO_ENV").as_deref() == Ok("test_postgres") {
            assert!(
                is_postgres,
                "test_postgres must provide a PostgreSQL DATABASE_URL"
            );
            return url;
        }
        if is_postgres { url } else { None }
    }

    #[test]
    fn sqlite_scheduler_lock_path_handles_uri_forms_and_queries() {
        let dir = tempdir().unwrap();
        let relative_parent = dir.path().join("relative");
        std::fs::create_dir(&relative_parent).unwrap();
        let database = relative_parent.join("data.db");
        let absolute_parent = dir.path().join("absolute");
        std::fs::create_dir(&absolute_parent).unwrap();
        let encoded_database = dir.path().join("encoded name.db");
        let encoded_path_uri = format!(
            "sqlite://{}?mode=rwc",
            encoded_database.to_string_lossy().replace(' ', "%20")
        );

        for uri in [
            "sqlite::memory:".to_string(),
            "sqlite://?mode=memory".to_string(),
            format!("sqlite://{}?mode=memory&mode=rw", database.display()),
            format!("sqlite://{}?mode=rw&mode=memory", database.display()),
        ] {
            let parsed = SqliteConnectOptions::from_str(&uri);
            assert_eq!(
                sqlite_scheduler_lock_path(&uri).is_ok(),
                parsed
                    .as_ref()
                    .is_ok_and(|options| !super::sqlite_options_are_in_memory(options))
            );
        }
        for uri in [
            "sqlite://db.sqlite?mode=bad&mode=rw",
            "sqlite://db.sqlite?mode=rw&mode=bad",
        ] {
            assert!(SqliteConnectOptions::from_str(uri).is_err());
            assert!(sqlite_scheduler_lock_path(uri).is_err());
        }
        let encoded_uri = format!("sqlite://{}?%6dode=%6demory&%6dode=rw", database.display());
        assert_eq!(
            sqlite_scheduler_lock_path(&encoded_uri).is_ok(),
            SqliteConnectOptions::from_str(&encoded_uri)
                .as_ref()
                .is_ok_and(|options| !super::sqlite_options_are_in_memory(options))
        );
        assert_eq!(
            sqlite_scheduler_lock_path(&format!(
                "sqlite://{}?mode=rwc&cache=shared",
                database.display()
            ))
            .unwrap()
            .file_name()
            .unwrap(),
            "data.db.scheduler.lock"
        );
        assert_eq!(
            sqlite_scheduler_lock_path(&format!(
                "sqlite://{}?mode=rw",
                absolute_parent.join("data.db").display()
            ))
            .unwrap()
            .parent()
            .unwrap(),
            absolute_parent
        );
        assert_eq!(
            sqlite_scheduler_lock_path(&encoded_path_uri)
                .unwrap()
                .file_name()
                .unwrap(),
            "encoded name.db.scheduler.lock"
        );
    }

    #[test]
    fn sqlite_scheduler_lock_is_non_blocking_and_releases() {
        let dir = tempdir().unwrap();
        let database = dir.path().join("app.sqlite");
        let uri = format!("sqlite://{}?mode=rwc", database.display());
        let first = acquire_sqlite_scheduler_lock(&uri, "unused")
            .unwrap()
            .unwrap();
        let second = acquire_sqlite_scheduler_lock(&uri, "unused").unwrap();
        assert!(second.is_none());
        if let super::SchedulerOwnership::Sqlite {
            file: Some(file), ..
        } = &first
        {
            unlock_sqlite_scheduler_file(file).unwrap();
        } else {
            panic!("expected SQLite ownership");
        }
        drop(first);
        assert!(
            acquire_sqlite_scheduler_lock(&uri, "unused")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn sqlite_scheduler_lock_works_with_separate_file_handles() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("app.sqlite.scheduler.lock");
        let first = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        lock_sqlite_scheduler_file(&first).unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(lock_sqlite_scheduler_file(&second).is_err());
        unlock_sqlite_scheduler_file(&first).unwrap();
        lock_sqlite_scheduler_file(&second).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_scheduler_lock_rejects_hard_linked_database() {
        let dir = tempdir().unwrap();
        let database = dir.path().join("app.sqlite");
        let hard_link = dir.path().join("alias.sqlite");
        std::fs::write(&database, b"").unwrap();
        std::fs::hard_link(&database, &hard_link).unwrap();
        let error =
            sqlite_scheduler_lock_path(&format!("sqlite://{}", database.display())).unwrap_err();
        assert!(error.contains("hard-linked"));
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_scheduler_lock_subprocess_aliases_contend_and_release() {
        let dir = tempdir().unwrap();
        let database = dir.path().join("app.sqlite");
        let alias = dir.path().join("alias.sqlite");
        std::fs::write(&database, b"").unwrap();
        std::os::unix::fs::symlink(&database, &alias).unwrap();
        let absolute_uri = format!("sqlite://{}?mode=rwc", database.display());
        let alias_uri = format!("sqlite://{}?mode=rwc", alias.display());

        for contender_uri in [absolute_uri, alias_uri] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "db::tests::sqlite_scheduler_lock_child",
                    "--nocapture",
                ])
                .current_dir(dir.path())
                .env(
                    "YORISHIRO_SQLITE_LOCK_CHILD_URI",
                    "sqlite://app.sqlite?mode=rwc",
                )
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let mut lines = std::io::BufReader::new(stdout).lines();
            let mut ready = false;
            for line in &mut lines {
                if line.is_ok_and(|line| line == "ready") {
                    ready = true;
                    break;
                }
            }
            assert!(ready);
            assert!(
                acquire_sqlite_scheduler_lock(&contender_uri, "unused")
                    .unwrap()
                    .is_none()
            );
            for _line in &mut lines {}
            assert!(child.wait().unwrap().success());
            let released = acquire_sqlite_scheduler_lock(&contender_uri, "unused")
                .unwrap()
                .unwrap();
            release_sqlite_for_test(&released);
        }
    }

    #[test]
    fn sqlite_scheduler_lock_child() {
        let Some(uri) = std::env::var_os("YORISHIRO_SQLITE_LOCK_CHILD_URI") else {
            return;
        };
        let ownership = acquire_sqlite_scheduler_lock(uri.to_str().unwrap(), "unused")
            .unwrap()
            .unwrap();
        println!("ready");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        release_sqlite_for_test(&ownership);
    }

    fn release_sqlite_for_test(ownership: &super::SchedulerOwnership) {
        if let super::SchedulerOwnership::Sqlite {
            file: Some(file), ..
        } = ownership
        {
            unlock_sqlite_scheduler_file(file).unwrap();
        } else {
            panic!("expected SQLite ownership");
        }
    }

    #[tokio::test]
    async fn postgres_scheduler_lock_contends_and_connection_close_releases() {
        let Some(url) = postgres_test_url() else {
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        let key = format!("test-scheduler-ownership-{}", uuid::Uuid::now_v7());
        let holder = super::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
        assert!(
            super::acquire_postgres_scheduler_lock(pool.clone(), &key)
                .await
                .unwrap()
                .is_none()
        );
        holder.release().await.unwrap();
        let final_ownership = super::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
        final_ownership.release().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn postgres_scheduler_detached_connection_close_releases_without_unlock() {
        let Some(url) = postgres_test_url() else {
            return;
        };

        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        let key = format!("test-scheduler-crash-release-{}", uuid::Uuid::now_v7());
        let mut ownership = super::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
        let (conn, held_key) = match &mut ownership {
            super::SchedulerOwnership::Postgres { conn, key } => {
                (conn.take().unwrap(), key.clone())
            }
            super::SchedulerOwnership::Sqlite { .. } => panic!("expected PostgreSQL ownership"),
        };
        drop(ownership);
        // Closing the detached session without an explicit unlock models a process crash.
        assert!(!held_key.is_empty());
        conn.close().await.unwrap();
        let final_ownership = super::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
        final_ownership.release().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn postgres_scheduler_lock_drop_releases_after_task_cancellation() {
        let Some(url) = postgres_test_url() else {
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        let key = format!("test-scheduler-cancel-release-{}", uuid::Uuid::now_v7());
        let ownership = super::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
        let task = tokio::spawn(async move {
            let _ownership = ownership;
            std::future::pending::<()>().await;
        });
        task.abort();
        let _ = task.await;
        let mut final_ownership = None;
        for _ in 0..20 {
            if let Some(ownership) = super::acquire_postgres_scheduler_lock(pool.clone(), &key)
                .await
                .unwrap()
            {
                final_ownership = Some(ownership);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        final_ownership
            .expect("cancellation must release scheduler ownership")
            .release()
            .await
            .unwrap();
        pool.close().await;
    }
}
