mod audit_log;
mod auth;
mod auth_sqlite;
mod entities;
mod import;
mod members;
mod queue;
mod schemas;
mod schemas_sqlite;
mod search;
mod setup;
mod system;
mod template_library;
mod workspaces;
mod workspaces_sqlite;

// The enterprise edition's own request suites are declared here because one crate has one
// integration-test binary.
// `ee_setup` is `ee/`'s own setup suite under a non-colliding name: base's `setup` is
// already declared above, and two modules of that name cannot coexist here.
mod dashboard;
mod ee_setup;
mod embedding;
mod entity_columns;
mod inference;
mod licence_gate;
mod marketplace;
mod oauth;
mod official_templates;
mod origin;
mod stripe;
mod tenant_auth;
mod worker_class;

/// `after_context` opens two pools Loco's own request-test harness knows nothing about: the identity pool and the tenant pool.
/// Leaving either open means a session survives on the throwaway test database, and `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving session.
/// `ctx.db` also needs closing: `config/test.yaml`'s `min_connections: 1` keeps one connection open from boot.
/// `queue_provider` is not closed here, and `bgworker::Queue` exposes no way to close one.
/// `config/test.yaml` has no `queue:` block, so it is `None` for every test that boots through `request_with_create_db`.
/// `queue.rs` is the exception, supplying its own SQLite queue config: that provider opens its own `sqlx::SqlitePool` against a file in a `TempDir` rather than a connection to the throwaway database, so it cannot hold the session that would fail `DROP DATABASE`, and the file goes with the `TempDir`.
///
/// Every request test that runs through `request_with_create_db` must call this before its closure returns.
pub(crate) async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    // Terminate lingering sessions before closing pools. `close()` on sqlx
    // pools doesn't terminate existing connections — it only prevents new
    // ones — so `DROP DATABASE` fails on "other sessions".
    //
    // `ctx.db.get_postgres_connection_pool()` is the pool that `ctx.db` uses,
    // which connects to the test database (e.g., `_loco_test_xxx`). Sessions
    // leaked by `request_with_create_db` hold connections on that database.
    // We acquire a connection from this pool, run `pg_terminate_backend`
    // (which only works on the same database), then close the pool.
    // Docker's postgres image creates `POSTGRES_USER` roles as superusers,
    // so `pg_terminate_backend` works in CI; on local dev it degrades
    // gracefully when the user lacks `pg_signal_backend` privileges.
    let test_pool = ctx.db.get_postgres_connection_pool();
    if let Ok(mut conn) = test_pool.acquire().await {
        let _ = sqlx::query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname = current_database()\
             AND pid <> pg_backend_pid()",
        )
        .execute(conn.as_mut())
        .await;
    }

    if let Some(db) = ctx.shared_store.get::<yorishiro::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    // Close ctx.db — it may hold active connections from transactions.
    ctx.db.close_by_ref().await.ok();
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

/// SQLite variant of loco's `request_with_create_db`.
///
/// Instead of `CREATE DATABASE` (which SQLite has no equivalent for), this generates a
/// unique temp file path, sets `YORISHIRO_TEST_CONFIG=test_sqlite`, overrides the database
/// URI to the generated file, then boots via `H::boot(StartMode::ServerOnly, ...)` — the
/// same path loco's own `boot_test_with_create_db` takes (load config, override URI, boot).
///
/// Cleanup closes `ctx.db` via `get_sqlite_connection_pool().close()`, removes the temp file
/// plus its `-wal`/`-shm`/`-journal` siblings, and restores any env vars set during the test.
/// Every test must call `close_app_pools_sqlite(&ctx, &db_path)` before its closure returns.
use axum_test::TestServer;
use std::net::SocketAddr;

/// SQLite variant of loco's `request_with_create_db`.
///
/// Instead of `CREATE DATABASE` (which SQLite has no equivalent for), this generates a
/// unique temp file path, sets `YORISHIRO_TEST_CONFIG=test_sqlite`, overrides the database
/// URI to the generated file, then boots via `H::boot(StartMode::ServerOnly, ...)` — the
/// same path loco's own `boot_test_with_create_db` takes (load config, override URI, boot).
///
/// Cleanup closes `ctx.db` via `get_sqlite_connection_pool().close()`, removes the temp file
/// plus its `-wal`/`-shm`/`-journal` siblings, and restores any env vars set during the test.
/// Every test must call `close_app_pools_sqlite(&ctx, &db_path)` before its closure returns.
#[allow(clippy::future_not_send)]
pub(crate) async fn request_with_create_sqlite<H: loco_rs::app::Hooks, F, Fut>(
    db_path: String,
    callback: F,
) where
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
