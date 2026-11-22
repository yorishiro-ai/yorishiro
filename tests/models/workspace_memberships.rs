use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use sea_orm::TransactionTrait;
use yorishiro::App;
use yorishiro::models::workspace_memberships::{self, WorkspaceRole};

#[tokio::test]
async fn direct_role_lookup_is_scoped_to_the_workspace() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let txn = ctx.db.begin().await.expect("begin grant");
        workspace_memberships::Model::grant(
            &txn,
            tenant.workspace_id,
            tenant.owner_id,
            WorkspaceRole::Viewer,
        )
        .await
        .expect("grant workspace access");
        txn.commit().await.expect("commit grant");
        assert!(matches!(
            workspace_memberships::Model::role(&ctx.db, tenant.workspace_id, tenant.owner_id)
                .await
                .expect("direct role"),
            Some(WorkspaceRole::Viewer)
        ));
        assert!(
            workspace_memberships::Model::role(&ctx.db, uuid::Uuid::new_v4(), tenant.owner_id)
                .await
                .expect("unrelated workspace")
                .is_none()
        );
    })
    .await;
}
