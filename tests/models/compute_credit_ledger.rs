use crate::requests::boot_request;
use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::ee::models::compute_credit_ledger as ledger;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;

#[tokio::test]
#[serial]
async fn actual_debit_never_overdraws_the_append_only_balance() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = tenant_tenants::ActiveModel {
            name: ActiveValue::Set("ledger-tenant".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert tenant");
        let workspace = workspace_workspaces::ActiveModel {
            tenant_id: ActiveValue::Set(tenant.id),
            name: ActiveValue::Set("ledger-workspace".into()),
            status: ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert workspace");

        ledger::earn(&ctx.db, workspace.id, 10)
            .await
            .expect("earn credits");
        let debit = ctx.db.begin().await.expect("begin debit transaction");
        ledger::debit_actual(&debit, workspace.id, 7)
            .await
            .expect("debit actual usage");
        debit.commit().await.expect("commit debit");
        assert_eq!(ledger::balance(&ctx.db, workspace.id).await.unwrap(), 3);

        let failed = ctx
            .db
            .begin()
            .await
            .expect("begin failed debit transaction");
        let error = ledger::debit_actual(&failed, workspace.id, 4)
            .await
            .expect_err("insufficient balance must refuse the debit");
        assert!(error.to_string().contains("insufficient compute credits"));
        failed.rollback().await.expect("rollback failed debit");
        assert_eq!(ledger::balance(&ctx.db, workspace.id).await.unwrap(), 3);
    })
    .await;
}
