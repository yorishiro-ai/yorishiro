use sqlx::Connection;
use sqlx::sqlite::SqliteConnectOptions;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::Write;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::Duration;
use tempfile::tempdir;

use serial_test::serial;
use sqlx::postgres::PgPoolOptions;
use yorishiro::edition::ee::db::{
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
            parsed.as_ref().is_ok_and(|options| {
                !yorishiro::edition::ee::db::sqlite_options_are_in_memory(options)
            })
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
            .is_ok_and(
                |options| !yorishiro::edition::ee::db::sqlite_options_are_in_memory(options)
            )
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
    if let yorishiro::edition::ee::db::SchedulerOwnership::Sqlite {
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
                "ee::db::sqlite_scheduler_lock_child",
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

fn release_sqlite_for_test(ownership: &yorishiro::edition::ee::db::SchedulerOwnership) {
    if let yorishiro::edition::ee::db::SchedulerOwnership::Sqlite {
        file: Some(file), ..
    } = ownership
    {
        unlock_sqlite_scheduler_file(file).unwrap();
    } else {
        panic!("expected SQLite ownership");
    }
}

#[tokio::test]
#[serial(process_environment)]
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
    let holder = yorishiro::edition::ee::db::acquire_postgres_scheduler_lock(pool.clone(), &key)
        .await
        .unwrap()
        .unwrap();
    assert!(
        yorishiro::edition::ee::db::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .is_none()
    );
    holder.release().await.unwrap();
    let final_ownership =
        yorishiro::edition::ee::db::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
    final_ownership.release().await.unwrap();
    pool.close().await;
}

#[tokio::test]
#[serial(process_environment)]
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
    let mut ownership =
        yorishiro::edition::ee::db::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
    let (conn, held_key) = match &mut ownership {
        yorishiro::edition::ee::db::SchedulerOwnership::Postgres { conn, key } => {
            (conn.take().unwrap(), key.clone())
        }
        yorishiro::edition::ee::db::SchedulerOwnership::Sqlite { .. } => {
            panic!("expected PostgreSQL ownership")
        }
    };
    drop(ownership);
    // Closing the detached session without an explicit unlock models a process crash.
    assert!(!held_key.is_empty());
    conn.close().await.unwrap();
    let final_ownership =
        yorishiro::edition::ee::db::acquire_postgres_scheduler_lock(pool.clone(), &key)
            .await
            .unwrap()
            .unwrap();
    final_ownership.release().await.unwrap();
    pool.close().await;
}

#[tokio::test]
#[serial(process_environment)]
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
    let ownership = yorishiro::edition::ee::db::acquire_postgres_scheduler_lock(pool.clone(), &key)
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
        if let Some(ownership) =
            yorishiro::edition::ee::db::acquire_postgres_scheduler_lock(pool.clone(), &key)
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
