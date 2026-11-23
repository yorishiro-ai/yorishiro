use crate::requests::boot_request_with;
use yorishiro::App;

/// This test is PostgreSQL-only: `lock_for_update` is a no-op on SQLite,
/// so the serialisation guarantee does not hold.
#[tokio::test]
async fn create_tenant_serializes_on_its_advisory_lock() {
    if !crate::require_postgres_backend() {
        return;
    }
    // The default pool is too small for a second connection to ever reach the advisory lock.
    boot_request_with::<App, _, _>(
        &[("database.max_connections", 4.into())],
        |_request, ctx| async move {
            let holder = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
            yorishiro::db::lock_for_update(&holder, "create_tenant")
                .await
                .unwrap();

            // A second caller must block behind the held lock, not proceed concurrently.
            let db = ctx.db.clone();
            let blocked = tokio::time::timeout(std::time::Duration::from_millis(500), async move {
                let txn = sea_orm::TransactionTrait::begin(&db).await.unwrap();
                let result = yorishiro::models::tenant_tenants::create_tenant(
                    &txn,
                    "blocked-racer",
                    Some(100),
                )
                .await;
                match &result {
                    Ok(_) => txn.commit().await.unwrap(),
                    Err(_) => txn.rollback().await.unwrap(),
                }
                result
            })
            .await;
            assert!(
                blocked.is_err(),
                "a second create_tenant call must block on the held lock, not proceed: {blocked:?}"
            );

            // Releasing the lock lets the next caller through.
            holder.rollback().await.unwrap();
            let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
            let result = yorishiro::models::tenant_tenants::create_tenant(
                &txn,
                "unblocked-racer",
                Some(100),
            )
            .await;
            assert!(result.is_ok(), "result: {result:?}");
            txn.commit().await.unwrap();
        },
    )
    .await;
}
