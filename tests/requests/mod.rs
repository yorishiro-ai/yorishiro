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
use std::net::SocketAddr;

/// SQLite variant of loco's `request_with_create_db`.
///
/// Instead of `CREATE DATABASE` (which SQLite has no equivalent for), this generates a
/// unique temp file path, sets `YORISHIRO_TEST_CONFIG=test_sqlite`, overrides the database
/// URI to the generated file, then boots via `H::boot(StartMode::ServerOnly, ...)` — the
/// same path loco's own `boot_test_with_create_db` takes (load config, override URI, boot).
///
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
