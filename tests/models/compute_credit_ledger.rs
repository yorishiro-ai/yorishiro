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
        ledger::earn(&ctx.db, workspace_id, 10).await.unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut debits = Vec::new();
        for _ in 0..2 {
            let db = ctx.db.clone();
            let barrier = barrier.clone();
            debits.push(tokio::spawn(async move {
                let txn = db.begin().await.unwrap();
                barrier.wait().await;
                if ledger::debit_actual(&txn, workspace_id, 7).await.is_ok() {
                    txn.commit().await.unwrap();
                    true
                } else {
                    txn.rollback().await.unwrap();
                    false
                }
            }));
        }
        let successful = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut successful = 0;
            for debit in debits {
                successful += u8::from(debit.await.unwrap());
            }
            successful
        })
        .await
        .expect("concurrent debits must finish");
        assert_eq!(successful, 1);
        assert_eq!(ledger::balance(&ctx.db, workspace_id).await.unwrap(), 3);
    })
    .await;
}
