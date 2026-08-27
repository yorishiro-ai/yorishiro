mod audit_log;
mod auth;
mod entities;
mod import;
mod members;
mod queue;
mod schemas;
mod search;
mod setup;
mod system;
mod template_library;
mod workspaces;

/// `after_context` opens two pools Loco's own request-test harness knows nothing about: the identity pool and the tenant pool.
/// Leaving either open means a session survives on the throwaway test database, and `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving session.
/// `ctx.db` also needs closing: `config/test.yaml`'s `min_connections: 1` keeps one connection open from boot.
/// `queue_provider` is not closed here, and `bgworker::Queue` exposes no way to close one.
/// `config/test.yaml` has no `queue:` block, so it is `None` for every test that boots through `request_with_create_db`.
/// `queue.rs` is the exception, supplying its own SQLite queue config: that provider opens its own `sqlx::SqlitePool` against a file in a `TempDir` rather than a connection to the throwaway database, so it cannot hold the session that would fail `DROP DATABASE`, and the file goes with the `TempDir`.
///
/// Every request test that runs through `request_with_create_db` must call this before its closure returns.
pub(crate) async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    if let Some(db) = ctx.shared_store.get::<yorishiro_core::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    ctx.db.get_postgres_connection_pool().close().await;
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
