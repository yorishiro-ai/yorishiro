//! Enterprise database primitives: scheduler ownership, so that exactly one replica runs a scheduled task at a time.
//!
//! PostgreSQL uses a detached session-scoped advisory lock, and SQLite uses a non-blocking lock file beside the database file.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, PgConnection, PgPool};

use crate::db::{AppContextBackend, DbHandle};

/// Ownership held for the complete scheduler task, including queue dispatch.
pub enum SchedulerOwnership {
    Postgres {
        conn: Option<PgConnection>,
        key: String,
    },
    Sqlite {
        file: Option<File>,
        path: PathBuf,
    },
}

impl SchedulerOwnership {
    pub(crate) fn sqlite_path(&self) -> Option<&Path> {
        match self {
            Self::Sqlite { path, .. } => Some(path),
            Self::Postgres { .. } => None,
        }
    }

    /// Explicitly releases ownership, then drops the detached session or file handle.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn release(mut self) -> Result<(), String> {
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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn acquire_scheduler_ownership(
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

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn acquire_postgres_scheduler_lock(
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

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn acquire_sqlite_scheduler_lock(
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

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn sqlite_scheduler_lock_path(uri: &str) -> Result<PathBuf, String> {
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

pub fn sqlite_options_are_in_memory(options: &SqliteConnectOptions) -> bool {
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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn lock_sqlite_scheduler_file(file: &File) -> std::io::Result<()> {
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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn unlock_sqlite_scheduler_file(file: &File) -> std::io::Result<()> {
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
fn lock_sqlite_scheduler_file(_file: &File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "SQLite scheduler file locking is unsupported on this platform",
    ))
}

#[cfg(not(unix))]
fn unlock_sqlite_scheduler_file(_file: &File) -> std::io::Result<()> {
    Ok(())
}
