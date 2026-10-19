use loco_rs::app::Hooks;
use sea_orm::ConnectionTrait;

#[tokio::test]
async fn shutdown_hook_keeps_database_available_until_loco_drains_workers() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let db = ctx.shared_store.get::<yorishiro::db::DbHandle>().unwrap();
        yorishiro::App::on_shutdown(&ctx).await;
        // Loco signals and awaits its queue runner only after this hook returns.
        assert!(!db.identity.is_closed());
        assert!(!db.tenant.pool().is_closed());
        ctx.db.execute_unprepared("SELECT 1").await.unwrap();
        for pool in [&db.identity, db.tenant.pool()] {
            sea_orm::sqlx::query("SELECT 1")
                .execute(pool)
                .await
                .unwrap();
        }
    })
    .await;
}
