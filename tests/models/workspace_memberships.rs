use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};
use yorishiro::App;
use yorishiro::models::workspace_memberships::{self, WorkspaceRole};
use yorishiro::models::{
    access, api_keys::ApiKeyScope, tenancy, tenant_memberships::MembershipRole,
};

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
        let inherited = access::workspace_access(
            &ctx.db,
            tenant.tenant_id,
            tenant.workspace_id,
            tenant.owner_id,
        )
        .await
        .expect("owner access")
        .expect("inherited authority");
        assert_eq!(inherited.max_scope, ApiKeyScope::Migration);
        assert_eq!(inherited.source, "tenant_role");
        assert!(inherited.workspace_admin);
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

#[tokio::test]
async fn workspace_access_requires_a_grant_and_caps_both_roles() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let txn = ctx.db.begin().await.expect("begin member");
        let member = tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &crate::requests::fixtures::unique_email("workspace-member"),
            "hunter2-hunter2",
            None,
            MembershipRole::Member,
        )
        .await
        .expect("register member");
        txn.commit().await.expect("commit member");
        assert!(
            access::workspace_access(&ctx.db, tenant.tenant_id, tenant.workspace_id, member.id,)
                .await
                .expect("ungranted member")
                .is_none()
        );
        for (role, cap, admin) in [
            (WorkspaceRole::Viewer, ApiKeyScope::Read, false),
            (WorkspaceRole::Member, ApiKeyScope::Write, false),
            (WorkspaceRole::Admin, ApiKeyScope::Write, true),
        ] {
            let txn = ctx.db.begin().await.expect("begin grant");
            workspace_memberships::Model::grant(&txn, tenant.workspace_id, member.id, role)
                .await
                .expect("grant role");
            txn.commit().await.expect("commit role");
            let grant =
                access::workspace_access(&ctx.db, tenant.tenant_id, tenant.workspace_id, member.id)
                    .await
                    .expect("resolve grant")
                    .expect("explicit access");
            assert_eq!(grant.max_scope, cap);
            assert_eq!(grant.source, "workspace_membership");
            assert_eq!(grant.workspace_admin, admin);
            assert!(
                access::require_tenant_admin(&ctx.db, tenant.tenant_id, Some(member.id))
                    .await
                    .is_err()
            );
        }
        assert!(
            access::workspace_access(
                &ctx.db,
                uuid::Uuid::new_v4(),
                tenant.workspace_id,
                member.id,
            )
            .await
            .expect("cross tenant")
            .is_none()
        );
    })
    .await;
}
