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
pub(crate) mod fill_defaults;
pub(crate) mod fixtures;
mod import;
#[cfg(feature = "enterprise")]
pub(crate) mod inference;
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
    let guard = crate::EnvGuard::capture(&[crate::env::TestEnv::MaxTenants.name()]);
    guard.set(crate::env::TestEnv::MaxTenants.name(), value);
    fut.await
}

use futures::FutureExt;
use loco_rs::TestServer;
use loco_rs::app::Hooks;
use sea_orm::sqlx::sqlite::SqliteConnectOptions;
use std::net::SocketAddr;
use std::str::FromStr;

/// Close every connection pool this app opens on a PostgreSQL test database.
///
/// `after_context` adds a tenant pool and shares the application pool with identity operations.
/// Leaving a pool open means a session survives on the throwaway test database,
/// and `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving session.
/// `ctx.db` also needs closing: `config/test_postgres.yaml`'s `min_connections: 1` keeps one connection open from boot.
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

/// Whether `DATABASE_URL` names a SQLite backend (file or in-memory).
///
/// Used by `boot_request` to dispatch to the correct boot path so the same
/// test code runs against either backend.
fn is_sqlite_backend() -> bool {
    let url = crate::env::TestEnv::DatabaseUrl.read().unwrap_or_default();
    url.starts_with("sqlite:")
}

/// Unified entry point for request tests across PostgreSQL and SQLite backends.
///
/// SQLite boots on its own database file per test.
/// PostgreSQL boots against the one migrated, shared database, so tests run in parallel without `CREATE DATABASE`/`DROP DATABASE`.
/// Enterprise fixtures create their own tenants; community fixtures share the single tenant and own their users and workspaces.
/// Neither edition may assume an empty database.
/// Tests of global state (first-run setup, maintenance mode, tenant cap, DB-wide monitors) use [`boot_request_isolated`].
///
/// The callback is wrapped in `catch_unwind` so the pool closer always runs, even if the callback panics.
#[allow(clippy::future_not_send)]
#[allow(clippy::extra_unused_type_parameters)]
pub(crate) async fn boot_request<H: Hooks, F, Fut>(callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let started = std::time::Instant::now();
    let name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    let _environment = crate::EnvironmentRead::enter();
    let admitted = std::time::Instant::now();
    if is_sqlite_backend() {
        record_request_timing(&name, "sqlite", &[("environment", admitted - started)]);
        boot_sqlite::<H, _, _>(callback).await;
    } else {
        request_with_shared_postgres::<H, _, _>(|request, ctx| {
            let booted = std::time::Instant::now();
            let result =
                std::panic::AssertUnwindSafe(callback(request, ctx.clone())).catch_unwind();
            async move {
                let outcome = result.await;
                let finished = std::time::Instant::now();
                close_app_pools(&ctx).await;
                record_request_timing(
                    &name,
                    "shared",
                    &[
                        ("environment", admitted - started),
                        ("boot", booted - admitted),
                        ("callback", finished - booted),
                        ("shutdown", finished.elapsed()),
                    ],
                );
                if let Err(panic) = outcome {
                    std::panic::resume_unwind(panic);
                }
            }
        })
        .await;
    }
}

/// Like [`boot_request`], but PostgreSQL gets a dedicated throwaway database.
///
/// For tests whose subject is database-wide or tenant-wide state that a shared database would make racy:
/// first-run setup, maintenance mode, tenant and workspace caps, membership enumeration, login workspace resolution, and DB-wide load monitoring.
/// Workers with different deployment providers or limits also need their own database and outbox.
#[allow(clippy::future_not_send)]
#[allow(clippy::extra_unused_type_parameters)]
pub(crate) async fn boot_request_isolated<H: Hooks, F, Fut>(callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let started = std::time::Instant::now();
    let name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    let _environment = crate::EnvironmentRead::enter();
    let admitted = std::time::Instant::now();
    if is_sqlite_backend() {
        record_request_timing(&name, "sqlite", &[("environment", admitted - started)]);
        boot_sqlite::<H, _, _>(callback).await;
    } else {
        let mut config = H::load_config(&loco_rs::environment::Environment::Test)
            .await
            .expect("load isolated postgres config");
        let database = loco_rs::testing::db::init_test_db_creation(&config.database.uri)
            .expect("prepare isolated database");
        database.init_db().await;
        let created = std::time::Instant::now();
        if let Some(loco_rs::config::QueueConfig::Postgres(queue)) = config.queue.as_mut()
            && queue.uri == config.database.uri
        {
            queue.uri = database.get_connection_str().to_owned();
        }
        config.database.uri = database.get_connection_str().to_owned();
        let outcome = std::panic::AssertUnwindSafe(async {
            let (request, ctx) = serve::<H>(config).await;
            let booted = std::time::Instant::now();
            let result = std::panic::AssertUnwindSafe(callback(request, ctx.clone()))
                .catch_unwind()
                .await;
            let finished = std::time::Instant::now();
            close_app_pools(&ctx).await;
            record_request_timing(
                &name,
                "isolated",
                &[
                    ("environment", admitted - started),
                    ("create", created - admitted),
                    ("boot", booted - created),
                    ("callback", finished - booted),
                    ("shutdown", finished.elapsed()),
                ],
            );
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        })
        .catch_unwind()
        .await;
        let cleanup = std::time::Instant::now();
        database.cleanup_db();
        record_request_timing(&name, "isolated", &[("cleanup", cleanup.elapsed())]);
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
}

fn record_request_timing(name: &str, scope: &str, phases: &[(&str, std::time::Duration)]) {
    let Some(directory) = crate::env::TestEnv::RequestTimingDir.read_os() else {
        return;
    };
    let rows = phases
        .iter()
        .map(|(phase, elapsed)| format!("{name}\t{scope}\t{phase}\t{:.6}\n", elapsed.as_secs_f64()))
        .collect::<String>();
    std::fs::write(
        std::path::Path::new(&directory).join(format!("{}.tsv", uuid::Uuid::now_v7())),
        rows,
    )
    .expect("write request phase timings");
}

#[allow(clippy::future_not_send)]
async fn boot_sqlite<H: Hooks, F, Fut>(callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let started = std::time::Instant::now();
    let name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    // Own the parent directory until boot, callback, pool shutdown, and cleanup all finish.
    // SQLite must not open a path whose parent can disappear during concurrent test teardown.
    let directory = tempfile::tempdir().expect("create sqlite test directory");
    let db_path = directory
        .path()
        .join(format!("yorishiro_test_{}.sqlite3", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    request_with_create_sqlite::<H, _, _>(db_path.clone(), |request, ctx| {
        let booted = std::time::Instant::now();
        let result = std::panic::AssertUnwindSafe(callback(request, ctx.clone())).catch_unwind();
        async move {
            let outcome = result.await;
            let finished = std::time::Instant::now();
            close_app_pools_sqlite(&ctx, &db_path).await;
            record_request_timing(
                &name,
                "sqlite",
                &[
                    ("boot", booted - started),
                    ("callback", finished - booted),
                    ("shutdown", finished.elapsed()),
                ],
            );
            if let Err(panic) = outcome {
                std::panic::resume_unwind(panic);
            }
        }
    })
    .await;
    drop(directory);
}

/// Applies the production migrations to the shared PostgreSQL test database once per test process.
///
/// Every booted app then skips `auto_migrate`: concurrent first migrations race on cluster-wide objects.
async fn migrate_shared_database() {
    use migration::MigratorTrait;
    static MIGRATED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    MIGRATED
        .get_or_init(|| async {
            let url = crate::env::TestEnv::DatabaseUrl
                .read()
                .expect("DATABASE_URL for the shared database");
            let db = sea_orm::Database::connect(&url)
                .await
                .expect("connect shared test database");
            migration::Migrator::up(&db, None)
                .await
                .expect("migrate shared test database");
            db.close().await.expect("close migration connection");
        })
        .await;
}

/// Boots the configured test app against the shared, already migrated PostgreSQL database.
#[allow(clippy::future_not_send)]
async fn request_with_shared_postgres<H: Hooks, F, Fut>(callback: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    migrate_shared_database().await;
    let mut config = H::load_config(&loco_rs::environment::Environment::Test)
        .await
        .expect("load postgres config");
    config.database.auto_migrate = false;
    let (server, ctx) = serve::<H>(config).await;
    callback(server, ctx).await;
}

/// Boots an isolated request app while temporarily overriding its configured timezone.
/// Timezone tests may mutate the database-wide timezone and maintenance state.
#[allow(clippy::future_not_send)]
pub(crate) async fn boot_request_with_timezone<H: Hooks, F, Fut>(
    timezone: &'static str,
    callback: F,
) where
    F: FnOnce(TestServer, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let guard = crate::EnvGuard::capture(&[crate::env::TestEnv::Timezone.name()]);
    guard.set(crate::env::TestEnv::Timezone.name(), timezone);
    boot_request_isolated::<H, _, _>(callback).await;
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
    let (server, ctx) = serve::<H>(config).await;
    callback(server, ctx).await;
}

/// Boots the app with a free port and wraps its router in a `TestServer`, as loco's own `request_internal` does.
#[allow(clippy::future_not_send)]
async fn serve<H: Hooks>(
    mut config: loco_rs::config::Config,
) -> (TestServer, loco_rs::app::AppContext) {
    if let Some(loco_rs::config::QueueConfig::Redis(queue)) = config.queue.as_mut() {
        static NEXT_DATABASE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let service = crate::env::TestEnv::RedisBaseUrl
            .read()
            .expect("Redis app fixtures require the matrix service URL");
        let database = NEXT_DATABASE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert!(database < 65536, "matrix Redis logical databases exhausted");
        let (base, _) = service.rsplit_once('/').expect("Redis database URL");
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
    crate::record_configured_topology(&boot.app_context);
    let routes = boot
        .router
        .clone()
        .expect("app must have routes after boot");
    let server = TestServer::new(routes.into_make_service_with_connect_info::<SocketAddr>())
        .expect("build TestServer");
    (server, boot.app_context.clone())
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

pub mod tenants;
