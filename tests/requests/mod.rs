mod sqlite;
#[cfg(feature = "enterprise")]
pub(crate) use sqlite::close_app_pools_sqlite;
use sqlite::{boot_sqlite, remove_sqlite_files, sqlite_queue_path};
mod api_keys;
mod audit_log;
mod auth;
mod entities;
mod export;
pub(crate) mod fill_defaults;
pub(crate) mod fixtures;
mod iam;
mod import;
pub(crate) mod mcp;
mod members;
mod schemas;
mod search;
mod setup;
mod template_library;
mod workspaces;

#[cfg(feature = "enterprise")]
fn licence_active(ctx: &loco_rs::app::AppContext) -> bool {
    use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
    ctx.shared_store
        .get::<std::sync::Arc<LicenceState>>()
        .is_some_and(|state| state.is_active())
}

/// Skip a test that needs more than the unlicensed one-tenant deployment cap.
///
/// Call it after boot, before the test creates its tenants.
/// It runs on the licensed CI job and on a local deployment with a valid `YORISHIRO_LICENSE_KEY`, and skips everywhere else.
#[cfg(feature = "enterprise")]
pub(crate) fn skip_unlicensed(ctx: &loco_rs::app::AppContext) -> bool {
    if licence_active(ctx) {
        return false;
    }
    assert!(
        std::env::var("YORISHIRO_LICENSE_KEY")
            .unwrap_or_default()
            .is_empty(),
        "YORISHIRO_LICENSE_KEY is set but the licence is not valid at boot"
    );
    eprintln!("skipping licensed test: set YORISHIRO_LICENSE_KEY to run it");
    true
}

/// Whether the booted app's queue provider is Redis (Valkey).
pub(crate) fn uses_redis_queue(ctx: &loco_rs::app::AppContext) -> bool {
    matches!(
        ctx.config.queue,
        Some(loco_rs::config::QueueConfig::Redis(_))
    )
}

/// Whether the booted app's queue provider is SQLite.
pub(crate) fn uses_sqlite_queue(ctx: &loco_rs::app::AppContext) -> bool {
    matches!(
        ctx.config.queue,
        Some(loco_rs::config::QueueConfig::Sqlite(_))
    )
}

/// One configuration change for a booted app: a dotted path into `Config` such as `settings.max_tenants`, and its value.
pub(crate) type Override = (&'static str, serde_json::Value);

/// Caps the tenants a booted app accepts.
pub(crate) fn max_tenants(limit: u64) -> Override {
    ("settings.max_tenants", limit.into())
}

/// Boots the app without a licence key, whatever the ambient environment holds.
pub(crate) fn unlicensed() -> Override {
    ("initializers.licence_key", "".into())
}

/// Applies the overrides to a freshly loaded configuration; the process environment is never touched.
fn apply_config_overrides(
    config: loco_rs::config::Config,
    overrides: &[Override],
) -> loco_rs::config::Config {
    if overrides.is_empty() {
        return config;
    }
    let mut document = serde_json::to_value(&config).expect("serialise config");
    for (path, value) in overrides {
        let mut node = &mut document;
        for key in path.split('.') {
            node = node
                .as_object_mut()
                .expect("config path crosses a non-object")
                .entry(key)
                .or_insert(serde_json::Value::Null);
            if node.is_null() {
                *node = serde_json::json!({});
            }
        }
        *node = value.clone();
    }
    serde_json::from_value(document).expect("overridden config stays valid")
}

use futures::FutureExt;
use loco_rs::TestServer;
use loco_rs::app::Hooks;
use std::net::SocketAddr;

/// Close every connection pool this app opens on a PostgreSQL test database.
///
/// `after_context` adds a tenant pool and shares the application pool with identity operations.
/// Leaving a pool open means a session survives on the throwaway test database,
/// and `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving session.
/// `ctx.db` also needs closing: `config/test.yaml`'s `min_connections: 1` keeps one connection open from boot.
/// Every request test that runs through `request_with_create_db` must call this before its closure returns.
pub(crate) async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    yorishiro::App::on_shutdown(ctx).await;
    if let Some(queue) = &ctx.queue_provider {
        queue.close().await.expect("close test queue");
    }
    if let Some(db) = ctx.shared_store.get::<yorishiro::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    ctx.db.clone().close().await.expect("close test database");
    if let Some(path) = sqlite_queue_path(&ctx.config) {
        remove_sqlite_files(&path);
    }
}

/// Whether `DATABASE_URL` names a SQLite backend (file or in-memory).
///
/// Used by `boot_request` to dispatch to the correct boot path so the same
/// test code runs against either backend.
fn is_sqlite_backend() -> bool {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    url.starts_with("sqlite:")
}

/// Entry point for request tests on either backend.
///
/// SQLite boots on its own database file per test.
/// PostgreSQL gets a throwaway database created from `template1` per test, so tests never share tenants or state.
/// The callback is wrapped in `catch_unwind` so pools are closed and the database dropped even if it panics.
/// Its future is boxed: nested boots (two contexts in one test) would otherwise keep every callback state machine on the test thread's small stack.
#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn boot_request<H: Hooks, F, Fut>(callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    boot_request_with::<H, _, _>(&[], callback).await;
}

/// `boot_request` with the configuration changed by `overrides`.
#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn boot_request_with<H: Hooks, F, Fut>(overrides: &[Override], callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    let _environment = crate::EnvironmentRead::enter();
    if is_sqlite_backend() {
        boot_sqlite::<H, _, _>(overrides, callback).await;
        return;
    }
    let mut config = H::load_config(&loco_rs::environment::Environment::Test)
        .await
        .map(|config| apply_config_overrides(config, overrides))
        .expect("load postgres config");
    let database = loco_rs::testing::db::init_test_db_creation(&config.database.uri)
        .expect("prepare test database");
    database.init_db().await;
    if let Some(loco_rs::config::QueueConfig::Postgres(queue)) = config.queue.as_mut()
        && queue.uri == config.database.uri
    {
        queue.uri = database.get_connection_str().to_owned();
    }
    config.database.uri = database.get_connection_str().to_owned();
    let outcome = std::panic::AssertUnwindSafe(async {
        let (request, ctx) = serve::<H>(config).await;
        let result = std::panic::AssertUnwindSafe(Box::pin(callback(request, ctx.clone())))
            .catch_unwind()
            .await;
        close_app_pools(&ctx).await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    })
    .catch_unwind()
    .await;
    database.cleanup_db();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Boots an isolated request app with its configured timezone overridden.
/// Timezone tests may mutate the database-wide timezone and maintenance state.
#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn boot_request_with_timezone<H: Hooks, F, Fut>(
    timezone: &'static str,
    callback: F,
) where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: Future<Output = ()>,
{
    boot_request_with::<H, _, _>(&[("settings.timezone", timezone.into())], callback).await;
}

/// Boots the app with a free port and wraps its router in a `TestServer`, as loco's own `request_internal` does.
async fn serve<H: Hooks>(
    mut config: loco_rs::config::Config,
) -> (TestServer, loco_rs::app::AppContext) {
    static NEXT_DATABASE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    if let Some(loco_rs::config::QueueConfig::Redis(queue)) = config.queue.as_mut() {
        // Each app gets its own logical Redis database on the configured server.
        let database = NEXT_DATABASE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(database < 65536, "matrix Redis logical databases exhausted");
        let uri = queue.uri.trim_end_matches('/');
        let base = match uri.rsplit_once('/') {
            Some((head, db)) if !db.is_empty() && db.bytes().all(|b| b.is_ascii_digit()) => head,
            _ => uri,
        };
        queue.uri = format!("{base}/{database}");
    }
    config.server.port = loco_rs::testing::prelude::get_available_port().await;
    let boot = H::boot(
        loco_rs::boot::StartMode::ServerOnly,
        &loco_rs::environment::Environment::Test,
        config,
    )
    .await
    .expect("boot app");
    let routes = boot
        .router
        .clone()
        .expect("app must have routes after boot");
    let server = TestServer::new(routes.into_make_service_with_connect_info::<SocketAddr>())
        .expect("build TestServer");
    (server, boot.app_context)
}

#[cfg(feature = "enterprise")]
/// The transaction a workspace-scoped model call runs in, the way the handlers open it: scoped through the tenant pool on PostgreSQL, on the application connection on SQLite.
pub(crate) async fn workspace_txn(
    ctx: &loco_rs::app::AppContext,
    tenant_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
) -> sea_orm::DatabaseTransaction {
    yorishiro::db::begin_workspace(ctx, tenant_id, workspace_id)
        .await
        .expect("begin workspace transaction")
}

pub(crate) mod tenants;
