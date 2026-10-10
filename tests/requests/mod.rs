mod api_keys;
mod audit_log;
mod auth;
#[cfg(feature = "enterprise")]
mod dashboard;
mod ee_setup;
#[cfg(feature = "enterprise")]
mod embedding;
mod entities;
#[cfg(feature = "enterprise")]
mod entity_columns;
mod fixtures;
mod import;
#[cfg(feature = "enterprise")]
mod inference;
#[cfg(feature = "enterprise")]
mod licence_gate;
#[cfg(feature = "enterprise")]
mod marketplace;
#[cfg(feature = "enterprise")]
mod mcp;
mod members;
#[cfg(feature = "enterprise")]
mod oauth;
#[cfg(feature = "enterprise")]
mod official_templates;
#[cfg(feature = "enterprise")]
mod openapi;
#[cfg(feature = "enterprise")]
mod origin;
pub(crate) mod query_worker;
#[cfg(feature = "enterprise")]
mod queue;
mod relations;
#[cfg(feature = "enterprise")]
mod schema_forks;
mod schemas;
mod search;
mod setup;
#[cfg(feature = "enterprise")]
mod stripe;
mod system;
mod template_library;
#[cfg(feature = "enterprise")]
mod tenant_auth;
#[cfg(feature = "enterprise")]
mod worker_class;
mod workspaces;

/// Sets `YORISHIRO_MAX_TENANTS` for the duration of the future.
pub(crate) async fn with_max_tenants<T>(
    value: &str,
    fut: impl std::future::Future<Output = T>,
) -> T {
    let guard = crate::EnvGuard::capture(&["YORISHIRO_MAX_TENANTS"]);
    guard.set("YORISHIRO_MAX_TENANTS", value);
    fut.await
}

use axum_test::TestServer;
use futures::FutureExt;
use loco_rs::app::Hooks;
use loco_rs::testing::prelude::*;
use serial_test::serial;
use sqlx::sqlite::SqliteConnectOptions;
use std::net::SocketAddr;
use std::str::FromStr;

/// Close every connection pool this app opens on a PostgreSQL test database.
///
/// `after_context` opens two pools Loco's own request-test harness knows nothing about:
/// the identity pool (eager) and the tenant pool (lazy).
/// Leaving either open means a session survives on the throwaway test database,
/// and `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving session.
/// `ctx.db` also needs closing: `config/test_postgres.yaml`'s `min_connections: 1` keeps one connection open from boot.
/// Every request test that runs through `request_with_create_db` must call this before its closure returns.
pub(crate) async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    if let Some(queue) = &ctx.queue_provider {
        queue.shutdown().expect("shutdown test queue");
    }
    if let Some(db) = ctx.shared_store.get::<yorishiro::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    ctx.db.get_postgres_connection_pool().close().await;
    if let Some(path) = sqlite_queue_path(&ctx.config) {
        remove_sqlite_files(&path);
    }
}

fn sqlite_queue_path(config: &loco_rs::config::Config) -> Option<String> {
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

fn remove_sqlite_files(path: &str) {
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
    assert!(!first.exists());
    assert!(second.exists());
    remove_sqlite_files(second.to_str().expect("UTF-8 queue path"));
    assert!(!second.exists());
}

/// SQLite variant of `close_app_pools`.
/// On SQLite `after_context` builds no `DbHandle` (no RLS, no second tenant),
/// so there is only `ctx.db` to close.
/// The configured queue provider is shut down before SQLite files are removed.
/// When a queue provider is configured, shutdown cancels its workers before file cleanup.
pub(crate) async fn close_app_pools_sqlite(ctx: &loco_rs::app::AppContext, db_path: &str) {
    if let Some(queue) = &ctx.queue_provider {
        queue.shutdown().expect("shutdown sqlite test queue");
    }
    ctx.db.get_sqlite_connection_pool().close().await;
    // Clean up the temp SQLite file and its journaling siblings.
    remove_sqlite_files(db_path);
}

pub(crate) fn is_sqlite_queue() -> bool {
    std::env::var("QUEUE_URL")
        .is_ok_and(|url| url.starts_with("sqlite://") || url.starts_with("sqlite::"))
}

/// Whether `DATABASE_URL` names a SQLite backend (file or in-memory).
///
/// Used by `boot_request` to dispatch to the correct boot path so the same
/// test code runs against either backend.
fn is_sqlite_backend() -> bool {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    url.starts_with("sqlite:")
}

/// Unified entry point for request tests across PostgreSQL and SQLite backends.
///
/// Detects the backend from `DATABASE_URL` and dispatches to
/// `request_with_create_db` (PostgreSQL) or `request_with_create_sqlite` (SQLite),
/// wrapping the callback in `catch_unwind` so that the appropriate pool closer
/// always runs — even if the callback panics.
///
/// All test files use this instead of calling either boot path directly.
#[allow(clippy::future_not_send)]
#[allow(clippy::extra_unused_type_parameters)]
pub(crate) async fn boot_request<H: Hooks, F, Fut>(callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if is_sqlite_backend() {
        // Own the parent directory until boot, callback, pool shutdown, and cleanup all finish.
        // SQLite must not open a path whose parent can disappear during concurrent test teardown.
        let directory = tempfile::tempdir().expect("create sqlite test directory");
        let db_path = directory
            .path()
            .join(format!("yorishiro_test_{}.sqlite3", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        request_with_create_sqlite::<H, _, _>(db_path.clone(), |request, ctx| {
            let result =
                std::panic::AssertUnwindSafe(callback(request, ctx.clone())).catch_unwind();
            async move {
                match result.await {
                    Ok(()) => {}
                    Err(panic) => {
                        close_app_pools_sqlite(&ctx, &db_path).await;
                        std::panic::resume_unwind(panic);
                    }
                }
                close_app_pools_sqlite(&ctx, &db_path).await;
            }
        })
        .await;
        drop(directory);
    } else {
        request_with_create_db::<H, _, _>(|request, ctx| {
            crate::record_configured_topology(&ctx);
            let result =
                std::panic::AssertUnwindSafe(callback(request, ctx.clone())).catch_unwind();
            async move {
                match result.await {
                    Ok(()) => {}
                    Err(panic) => {
                        close_app_pools(&ctx).await;
                        std::panic::resume_unwind(panic);
                    }
                }
                close_app_pools(&ctx).await;
            }
        })
        .await;
    }
}

/// SQLite variant of `boot_request`.
///
/// Boots the app through `request_with_create_sqlite`, then wraps the callback in
/// `catch_unwind` so that `close_app_pools_sqlite` always runs.
/// Re-throws the original panic afterward so the test reports the real failure message.
///
/// **Deprecated**: `boot_request` now dispatches automatically, so this path is
/// kept only for tests that need explicit control (e.g. `search.rs` which seeds
/// an in-memory database directly).
#[allow(clippy::future_not_send)]
#[allow(clippy::extra_unused_type_parameters)]
pub(crate) async fn boot_request_sqlite<H: Hooks, F, Fut>(db_path: String, callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    request_with_create_sqlite::<H, _, _>(db_path.clone(), |request, ctx| {
        let result = std::panic::AssertUnwindSafe(callback(request, ctx.clone())).catch_unwind();
        async move {
            match result.await {
                Ok(()) => {}
                Err(panic) => {
                    close_app_pools_sqlite(&ctx, &db_path).await;
                    std::panic::resume_unwind(panic);
                }
            }
            close_app_pools_sqlite(&ctx, &db_path).await;
        }
    })
    .await;
}

/// SQLite variant of loco's `request_with_create_db`.
///
/// Instead of `CREATE DATABASE` (which SQLite has no equivalent for), this generates a
/// unique temp file path, overrides the database URI to the generated file, then boots via
/// `H::boot(StartMode::ServerOnly, ...)`.
#[allow(clippy::future_not_send)]
pub(crate) async fn request_with_create_sqlite<H: Hooks, F, Fut>(db_path: String, callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // Load the configured test app, then override only its isolated database and queue files.
    let mut config = H::load_config(&loco_rs::environment::Environment::Test)
        .await
        .expect("load sqlite config");
    config.database.uri = format!("sqlite://{}?mode=rwc", db_path);
    if let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() {
        let queue_path = std::path::Path::new(&db_path)
            .with_file_name(format!("{}_queue.sqlite3", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        queue.uri = format!("sqlite://{queue_path}?mode=rwc");
    }
    // Override the server port so tests don't collide with each other.
    let port = loco_rs::testing::prelude::get_available_port().await;
    config.server.port = port;

    let boot = H::boot(
        loco_rs::boot::StartMode::ServerOnly,
        &loco_rs::environment::Environment::Test,
        config,
    )
    .await
    .expect("boot sqlite app");
    assert!(
        boot.app_context.queue_provider.is_some(),
        "booted app has no queue provider"
    );
    crate::record_configured_topology(&boot.app_context);

    // Build the TestServer from the app's router, using the same pattern as
    // loco's own `request_internal`.
    let routes = boot
        .router
        .clone()
        .expect("app must have routes after boot");
    let server = TestServer::new(routes.into_make_service_with_connect_info::<SocketAddr>())
        .expect("build TestServer");

    callback(server, boot.app_context.clone()).await;
}

#[tokio::test]
#[serial(postgres_sqlite_queue)]
async fn postgres_sqlite_queue_boots_clean_up_before_the_next_boot() {
    if !crate::require_postgres_backend() || !is_sqlite_queue() {
        return;
    }

    let queue_path = std::sync::Arc::new(std::sync::Mutex::new(None));
    for _ in 0..2 {
        let prior = queue_path.clone();
        boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = ctx.config.queue.as_ref()
            else {
                return;
            };
            let path = sqlite_queue_path(&ctx.config).expect("SQLite queue path");
            if let Some(previous) = prior.lock().expect("queue path lock").as_ref() {
                assert_eq!(previous, &path, "the configured regression queue is stable");
            }
            assert!(ctx.queue_provider.is_some(), "SQLite queue did not boot");
            std::fs::write(&path, format!("queue={}", queue.uri)).expect("write queue artifact");
            *prior.lock().expect("queue path lock") = Some(path);
        })
        .await;

        let path = queue_path
            .lock()
            .expect("queue path lock")
            .clone()
            .expect("queue path after boot");
        assert!(
            !std::path::Path::new(&path).exists(),
            "queue file leaked after shutdown"
        );
        assert!(!std::path::Path::new(&format!("{path}-wal")).exists());
        assert!(!std::path::Path::new(&format!("{path}-shm")).exists());
        assert!(!std::path::Path::new(&format!("{path}-journal")).exists());
    }
}
