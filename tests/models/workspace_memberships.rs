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
            tenancy::list_workspaces_for_user(&ctx.db, member.id)
                .await
                .expect("discover ungranted workspaces")
                .is_empty()
        );
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
            let candidates = tenancy::list_workspaces_for_user(&ctx.db, member.id)
                .await
                .expect("discover granted workspaces");
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].id, tenant.workspace_id);
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
        let key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            Some(member.id),
            false,
        )
        .await
        .expect("issue attributed key");
        let txn = ctx.db.begin().await.expect("lower workspace grant");
        workspace_memberships::Model::grant(
            &txn,
            tenant.workspace_id,
            member.id,
            WorkspaceRole::Viewer,
        )
        .await
        .expect("lower grant");
        txn.commit().await.expect("commit lower grant");
        let preview = request
            .post(&format!("/api/members/{}/role-preview", member.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"role":"member"}))
            .await;
        assert_eq!(preview.status_code(), 200);
        assert_eq!(
            preview.json::<serde_json::Value>()["keys"][0]["effective_scope"],
            "read"
        );
        let removal = request
            .get(&format!("/api/members/{}/removal-preview", member.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(removal.status_code(), 200);
        assert_eq!(
            removal.json::<serde_json::Value>()["lost_workspace_ids"],
            serde_json::json!([tenant.workspace_id])
        );
        let too_broad = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Write,
            Some(member.id),
            false,
        )
        .await;
        assert!(matches!(
            too_broad,
            Err(yorishiro::YorishiroError::ScopeInsufficient { .. })
        ));
        let identity = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", key.plaintext))
            .await;
        assert_eq!(identity.status_code(), 200);
        assert_eq!(identity.json::<serde_json::Value>()["scope"], "read");
        let txn = ctx.db.begin().await.expect("begin revocation");
        workspace_memberships::Model::revoke(&txn, tenant.workspace_id, member.id)
            .await
            .expect("revoke grant");
        txn.commit().await.expect("commit revocation");
        let denied = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", key.plaintext))
            .await;
        assert_eq!(denied.status_code(), 401);
        assert!(
            yorishiro::models::api_keys::Entity::create_api_key(
                &ctx.db,
                tenant.workspace_id,
                ApiKeyScope::Read,
                Some(member.id),
                false,
            )
            .await
            .is_err()
        );
        assert!(
            tenancy::list_workspaces_for_user(&ctx.db, member.id)
                .await
                .expect("discover after revocation")
                .is_empty()
        );
        assert!(
            access::workspace_access(&ctx.db, tenant.tenant_id, tenant.workspace_id, member.id,)
                .await
                .expect("revoked access")
                .is_none()
        );
        let txn = ctx.db.begin().await.expect("begin restoration");
        workspace_memberships::Model::grant(
            &txn,
            tenant.workspace_id,
            member.id,
            WorkspaceRole::Member,
        )
        .await
        .expect("restore grant");
        txn.commit().await.expect("commit restoration");
        assert!(
            yorishiro::models::api_keys::Entity::find_by_id(key.id)
                .one(&ctx.db)
                .await
                .expect("lookup revoked key")
                .expect("revoked grant history retained")
                .revoked_at
                .is_some()
        );
        yorishiro::models::tenant_memberships::remove_member(&ctx.db, tenant.tenant_id, member.id)
            .await
            .expect("remove tenant membership and grants");
        assert!(
            workspace_memberships::Model::role(&ctx.db, tenant.workspace_id, member.id)
                .await
                .expect("removed workspace grant")
                .is_none()
        );
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant.tenant_id,
            member.id,
            MembershipRole::Member,
        )
        .await
        .expect("restore tenant membership only");
        let login = request
            .post("/auth/login")
            .json(&serde_json::json!({"email":member.email,"password":"hunter2-hunter2"}))
            .await;
        assert_eq!(login.status_code(), 403);
        assert_eq!(
            login.json::<serde_json::Value>()["error"]["code"],
            "workspace_access_required"
        );
        assert!(
            tenancy::list_workspaces_for_user(&ctx.db, member.id)
                .await
                .expect("restoration must not resurrect workspace grants")
                .is_empty()
        );
    })
    .await;
}
