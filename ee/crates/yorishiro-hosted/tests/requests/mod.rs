mod dashboard;

/// `after_context` opens two pools Loco's own request-test harness knows nothing about (the
/// identity pool, eager, and the tenant pool, lazy): see `yorishiro_core`'s own
/// `tests/requests/mod.rs` doc comment for why closing them is required before
/// `request_with_create_db`'s `DROP DATABASE` teardown.
pub(crate) async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    if let Some(db) = ctx.shared_store.get::<yorishiro_core::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    ctx.db.get_postgres_connection_pool().close().await;
}
