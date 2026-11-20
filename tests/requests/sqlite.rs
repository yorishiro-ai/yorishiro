use std::future::Future;
use std::str::FromStr;

use futures::FutureExt;
use loco_rs::{TestServer, app::Hooks};
use sea_orm::sqlx::sqlite::SqliteConnectOptions;

use super::{Override, apply_config_overrides, serve};

pub(super) fn sqlite_queue_path(config: &loco_rs::config::Config) -> Option<String> {
    let loco_rs::config::QueueConfig::Sqlite(queue) = config.queue.as_ref()? else {
        return None;
    };
    let filename = SqliteConnectOptions::from_str(&queue.uri)
        .ok()?
        .get_filename()
        .to_string_lossy()
        .into_owned();
    (!filename.is_empty()).then_some(filename)
}

pub(super) fn remove_sqlite_files(path: &str) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
}

#[test]
fn sqlite_queue_cleanup_removes_database_and_journal_siblings() {
    let directory = tempfile::tempdir().expect("create queue cleanup directory");
    let first = directory.path().join("first.sqlite3");
    let second = directory.path().join("second.sqlite3");
    for path in [&first, &second] {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            std::fs::write(format!("{}{suffix}", path.display()), b"test")
                .expect("create queue cleanup fixture");
        }
    }
    assert_ne!(first, second);
    remove_sqlite_files(first.to_str().expect("UTF-8 queue path"));
    for suffix in ["", "-wal", "-shm", "-journal"] {
        assert!(!std::path::Path::new(&format!("{}{suffix}", first.display())).exists());
        assert!(std::path::Path::new(&format!("{}{suffix}", second.display())).exists());
    }
    remove_sqlite_files(second.to_str().expect("UTF-8 queue path"));
    for suffix in ["", "-wal", "-shm", "-journal"] {
        assert!(!std::path::Path::new(&format!("{}{suffix}", second.display())).exists());
    }
}

/// SQLite variant of `close_app_pools`.
/// On SQLite `after_context` builds no `DbHandle` (no RLS, no second tenant),
/// so there is only `ctx.db` to close.
/// The configured queue provider is shut down before SQLite files are removed.
/// When a queue provider is configured, shutdown cancels its workers before file cleanup.
pub(crate) async fn close_app_pools_sqlite(ctx: &loco_rs::app::AppContext, db_path: &str) {
    yorishiro::App::on_shutdown(ctx).await;
    if let Some(queue) = &ctx.queue_provider {
        queue.close().await.expect("close sqlite test queue");
    }
    ctx.db.get_sqlite_connection_pool().close().await;
    if let Some(path) = sqlite_queue_path(&ctx.config) {
        remove_sqlite_files(&path);
    }
    remove_sqlite_files(db_path);
}

pub(super) async fn boot_sqlite<H: Hooks, F, Fut>(overrides: &[Override], callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    // Own the parent directory until boot, callback, pool shutdown, and cleanup all finish.
    // SQLite must not open a path whose parent can disappear during concurrent test teardown.
    let directory = tempfile::tempdir().expect("create sqlite test directory");
    let db_path = directory
        .path()
        .join(format!("yorishiro_test_{}.sqlite3", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    request_with_create_sqlite::<H, _, _>(db_path.clone(), overrides, |request, ctx| {
        let result =
            std::panic::AssertUnwindSafe(Box::pin(callback(request, ctx.clone()))).catch_unwind();
        async move {
            let outcome = result.await;
            close_app_pools_sqlite(&ctx, &db_path).await;
            if let Err(panic) = outcome {
                std::panic::resume_unwind(panic);
            }
        }
    })
    .await;
    drop(directory);
}

/// SQLite variant of loco's `request_with_create_db`.
///
/// Instead of `CREATE DATABASE` (which SQLite has no equivalent for), this generates a
/// unique temp file path, overrides the database URI to the generated file, then boots via
/// `H::boot(StartMode::ServerOnly, ...)`.
async fn request_with_create_sqlite<H: Hooks, F, Fut>(
    db_path: String,
    overrides: &[Override],
    callback: F,
) where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    // Load the configured test app, then override only its isolated database and queue files.
    let mut config = H::load_config(&loco_rs::environment::Environment::Test)
        .await
        .map(|config| apply_config_overrides(config, overrides))
        .expect("load sqlite config");
    config.database.uri = format!("sqlite://{db_path}?mode=rwc");
    if let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() {
        let queue_path = std::path::Path::new(&db_path)
            .with_file_name(format!("{}_queue.sqlite3", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        queue.uri = format!("sqlite://{queue_path}?mode=rwc");
    }
    let (server, ctx) = serve::<H>(config).await;
    Box::pin(callback(server, ctx)).await;
}
