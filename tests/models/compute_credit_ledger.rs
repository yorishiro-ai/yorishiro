use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use sea_orm::TransactionTrait;
use yorishiro::App;
use yorishiro::edition::ee::models::compute_credit_ledger as ledger;

#[tokio::test]
async fn actual_debit_never_overdraws_the_append_only_balance() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let workspace_id = fixture.workspace_id;

        ledger::earn(&ctx.db, workspace_id, 10)
            .await
            .expect("earn credits");

        // Both transactions reach the debit together. PostgreSQL serializes them with the advisory
        // lock, while SQLite serializes the write transaction and rejects a stale writer as a whole.
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let first_db = ctx.db.clone();
        let second_db = ctx.db.clone();
        let first_barrier = barrier.clone();
        let second_barrier = barrier;
        let first = tokio::spawn(async move {
            let txn = first_db.begin().await.expect("begin first debit");
            first_barrier.wait().await;
            let result = ledger::debit_actual(&txn, workspace_id, 7).await;
            if result.is_ok() {
                txn.commit().await.expect("commit first debit");
                true
            } else {
                txn.rollback().await.expect("rollback first debit");
                false
            }
        });
        let second = tokio::spawn(async move {
            let txn = second_db.begin().await.expect("begin second debit");
            second_barrier.wait().await;
            let result = ledger::debit_actual(&txn, workspace_id, 7).await;
            if result.is_ok() {
                txn.commit().await.expect("commit second debit");
                true
            } else {
                txn.rollback().await.expect("rollback second debit");
                false
            }
        });
        let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            (
                first.await.expect("first debit task"),
                second.await.expect("second debit task"),
            )
        })
        .await
        .expect("concurrent debit barrier must release");
        assert_eq!(u8::from(first) + u8::from(second), 1);
        assert_eq!(ledger::balance(&ctx.db, workspace_id).await.unwrap(), 3);
    })
    .await;
}
