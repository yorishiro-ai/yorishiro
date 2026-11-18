use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};
use yorishiro::App;
use yorishiro::models::workspace_memberships::{self, WorkspaceRole};

#[tokio::test]
async fn workspace_memberships_round_trip_roles() {
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
        let stored = workspace_memberships::Entity::find()
            .filter(workspace_memberships::Column::WorkspaceId.eq(tenant.workspace_id))
            .filter(workspace_memberships::Column::UserId.eq(tenant.owner_id))
            .one(&ctx.db)
            .await
            .expect("read membership")
            .expect("persisted membership");
        assert_eq!(stored.workspace_id, tenant.workspace_id);
        assert_eq!(stored.user_id, tenant.owner_id);
        assert_eq!(stored.role, "viewer");
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
        workspace_memberships::Entity::delete_by_id(stored.id)
            .exec(&ctx.db)
            .await
            .expect("delete membership");
        assert!(
            workspace_memberships::Model::role(&ctx.db, tenant.workspace_id, tenant.owner_id)
                .await
                .expect("removed grant")
                .is_none()
        );
    })
    .await;
}
