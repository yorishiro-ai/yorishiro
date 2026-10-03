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

#[cfg(test)]
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

    use super::{
        acquire_sqlite_scheduler_lock, lock_sqlite_scheduler_file, sqlite_scheduler_lock_path,
        unlock_sqlite_scheduler_file,
    };
    use sqlx::postgres::PgPoolOptions;

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
                    "ee::db::tests::sqlite_scheduler_lock_child",
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
