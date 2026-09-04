mod audit_log;
mod auth;
mod auth_sqlite;
mod dashboard;
mod ee_setup;
mod embedding;
mod entities;
mod entity_columns;
mod import;
mod inference;
mod licence_gate;
mod marketplace;
mod members;
mod oauth;
mod official_templates;
mod origin;
mod queue;
mod schemas;
mod schemas_sqlite;
mod search;
mod setup;
mod stripe;
mod system;
mod template_library;
mod tenant_auth;
mod worker_class;
mod workspaces;
mod workspaces_sqlite;

/// Sets `YORISHIRO_MAX_TENANTS` for the duration of the future, restoring whatever was there before.
pub(crate) async fn with_max_tenants<T>(
    value: &str,
    fut: impl std::future::Future<Output = T>,
) -> T {
    let previous = std::env::var("YORISHIRO_MAX_TENANTS").ok();
    // SAFETY: serialized by every test in this binary being #[serial] on the default key.
    unsafe {
        std::env::set_var("YORISHIRO_MAX_TENANTS", value);
    }
    let result = fut.await;
    unsafe {
        match &previous {
            Some(v) => std::env::set_var("YORISHIRO_MAX_TENANTS", v),
            None => std::env::remove_var("YORISHIRO_MAX_TENANTS"),
        }
    }
    result
}

use axum_test::TestServer;
use futures::FutureExt;
use loco_rs::app::Hooks;
use loco_rs::testing::prelude::*;
use std::net::SocketAddr;
use yorishiro::app::App;

/// Close every connection pool this app opens on a PostgreSQL test database.
///
/// `after_context` opens two pools Loco's own request-test harness knows nothing about:
/// the identity pool (eager) and the tenant pool (lazy).
/// Leaving either open means a session survives on the throwaway test database,
/// and `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving session.
/// `ctx.db` also needs closing: `config/test.yaml`'s `min_connections: 1` keeps one connection open from boot.
///
/// `spawn_startup_reindex` spawns a background task that holds a connection from `ctx.db` for its entire lifetime.
/// Without shutdown-and-await, that task would still hold a session when pools are closed,
/// causing `DROP DATABASE` to panic with "being accessed by other users".
/// `close_app_pools` signals shutdown and awaits the task *before* closing pools.
///
/// Every request test that runs through `request_with_create_db` must call this before its closure returns.
pub(crate) async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    // Signal shutdown to the startup reindex background task and await its actual
    // completion before closing pools. This structurally closes the race: if the task
    // is mid-await when signaled, we wait for that await to return (at which point it
    // sees the flag and exits) rather than closing pools while the task still holds a
    // ctx.db connection.
    if let Some(handle) = ctx
        .shared_store
        .remove::<yorishiro::app::StartupReindexHandle>()
    {
        handle.shutdown_and_wait().await;
    }
    if let Some(db) = ctx.shared_store.get::<yorishiro::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    ctx.db.get_postgres_connection_pool().close().await;
}

/// SQLite variant of `close_app_pools`.
/// On SQLite `after_context` builds no `DbHandle` (no RLS, no second tenant),
/// so there is only `ctx.db` to close.
/// `queue:` is active in `config/test_sqlite.yaml` (the queue provider uses its own
/// pool, so it never holds the session that would fail `DROP DATABASE`);
/// `config/test_sqlite.yaml` has no `queue:` block, so it is `None` for every test that boots
/// through `request_with_create_db`.
/// `queue_provider` is not closed here, and `bgworker::Queue` exposes no way to close one.
pub(crate) async fn close_app_pools_sqlite(ctx: &loco_rs::app::AppContext, db_path: &str) {
    ctx.db.get_sqlite_connection_pool().close().await;
    // Clean up the temp SQLite file and its journaling siblings.
    let _ = std::fs::remove_file(db_path);
    let _ = std::fs::remove_file(format!("{db_path}-wal"));
    let _ = std::fs::remove_file(format!("{db_path}-shm"));
    let _ = std::fs::remove_file(format!("{db_path}-journal"));
}

/// Unified entry point for all PostgreSQL-backed request tests.
///
/// Boots the app through `request_with_create_db`, then wraps the callback in
/// `catch_unwind` so that `close_app_pools` always runs — even if the callback panics.
/// The original panic (assertion failure, etc.) is re-thrown afterward so the test
/// reports the real failure message, not a wrapper artifact.
///
/// All test files must use this instead of calling `request_with_create_db` directly.
#[allow(clippy::future_not_send)]
pub(crate) async fn boot_request<H: Hooks, F, Fut>(callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    request_with_create_db::<App, _, _>(|request, ctx| {
        let result = std::panic::AssertUnwindSafe(callback(request, ctx.clone())).catch_unwind();
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

/// SQLite variant of `boot_request`.
///
/// Boots the app through `request_with_create_sqlite`, then wraps the callback in
/// `catch_unwind` so that `close_app_pools_sqlite` always runs.
/// Re-throws the original panic afterward so the test reports the real failure message.
#[allow(clippy::future_not_send)]
pub(crate) async fn boot_request_sqlite<H: Hooks, F, Fut>(db_path: String, callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    request_with_create_sqlite::<App, _, _>(db_path.clone(), |request, ctx| {
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
/// unique temp file path, sets `YORISHIRO_TEST_CONFIG=test_sqlite`, overrides the database
/// URI to the generated file, then boots via `H::boot(StartMode::ServerOnly, ...)` — the
/// same path loco's own `boot_test_with_create_db` takes (load config, override URI, boot).
///
#[allow(clippy::future_not_send)]
pub(crate) async fn request_with_create_sqlite<H: Hooks, F, Fut>(db_path: String, callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // Load config via the App's override, which redirects Environment::Test → test_sqlite.yaml.
    let mut config = H::load_config(&loco_rs::environment::Environment::Test)
        .await
        .expect("load sqlite config");
    config.database.uri = format!("sqlite://{}?mode=rwc", db_path);
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

    // Build the TestServer from the app's router, using the same pattern as
    // loco's own `request_internal`.
    let routes = boot
        .router
        .clone()
        .expect("app must have routes after boot");
    let server = TestServer::new_with_config(
        routes.into_make_service_with_connect_info::<SocketAddr>(),
        loco_rs::testing::prelude::RequestConfig::default(),
    )
    .expect("build TestServer");

    callback(server, boot.app_context.clone()).await;
}
