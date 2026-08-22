mod auth;
mod members;
mod schemas;
mod system;
mod workspaces;

/// `after_context` opens two pools Loco's own request-test harness knows nothing about: the
/// identity pool (eager, holds a session immediately) and the tenant pool (lazy, only opened on
/// first use). Neither closing means a session survives on the throwaway test database, and
/// `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving
/// session. `ctx.db` itself also needs closing: `config/test.yaml`'s `min_connections: 1` keeps
/// one connection open from boot. `queue_provider` is not closed here: `config/test.yaml` has no
/// `queue:` block (see its comment), so it is always `None` in this environment; the Postgres
/// queue provider's `shutdown()` only cancels its polling loop, not its own `PgPool`, and has no
/// method that does, which is exactly why the queue stays off for tests rather than being closed
/// here.
///
/// Every request test that runs through `request_with_create_db` must call this before its
/// closure returns.
pub(crate) async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    if let Some(db) = ctx.shared_store.get::<yorishiro_core::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    ctx.db.get_postgres_connection_pool().close().await;
}
