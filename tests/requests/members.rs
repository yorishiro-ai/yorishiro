use super::boot_request;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

/// Creates a tenant and one active workspace, for tests that need somewhere to attach members.
async fn setup_tenant(ctx: &loco_rs::app::AppContext, name: &str) -> (uuid::Uuid, uuid::Uuid) {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(name.to_string()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");

    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set(format!("{name}-ws")),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");

    (tenant.id, workspace.id)
}

async fn issue_key_for(
    ctx: &loco_rs::app::AppContext,
    workspace_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> String {
    identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace_id,
        ApiKeyScope::Migration,
        Some(user_id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext
}

#[tokio::test]
#[serial]
async fn owner_can_list_and_add_members() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_id) = setup_tenant(&ctx, "acme").await;

        let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
            .await
            .expect("create owner");
        tenancy::add_member(&ctx.db, tenant_id, owner.id, MembershipRole::Owner)
            .await
            .expect("add owner");
        let owner_key = issue_key_for(&ctx, workspace_id, owner.id).await;

        // The invitee must already have an account before they can be added by email.
        let invitee = tenancy::create_user(&ctx.db, "invitee@example.com", "hunter2-hunter2", None)
            .await
            .expect("create invitee");

        let response = request
            .post("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "email": "invitee@example.com",
                "role": "member",
            }))
            .await;
        assert_eq!(response.status_code(), 201);
        let body: serde_json::Value = response.json();
        assert_eq!(body["user_id"], invitee.id.to_string());
        assert_eq!(body["role"], "member");

        let response = request
            .get("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        let emails: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["email"].as_str().unwrap())
            .collect();
        assert!(emails.contains(&"owner@example.com"));
        assert!(emails.contains(&"invitee@example.com"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn add_member_rejects_an_email_with_no_account() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_id) = setup_tenant(&ctx, "acme").await;

        let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
            .await
            .expect("create owner");
        tenancy::add_member(&ctx.db, tenant_id, owner.id, MembershipRole::Owner)
            .await
            .expect("add owner");
        let owner_key = issue_key_for(&ctx, workspace_id, owner.id).await;

        let response = request
            .post("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "email": "nobody@example.com",
                "role": "member",
            }))
            .await;
        assert_eq!(response.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn member_role_cannot_manage_members() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_id) = setup_tenant(&ctx, "acme").await;

        let member = tenancy::create_user(&ctx.db, "member@example.com", "hunter2-hunter2", None)
            .await
            .expect("create member");
        tenancy::add_member(&ctx.db, tenant_id, member.id, MembershipRole::Member)
            .await
            .expect("add member");
        let member_key = issue_key_for(&ctx, workspace_id, member.id).await;

        let response = request
            .get("/api/members")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), 403);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_endpoints_require_authentication() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/members").await;
        assert_eq!(response.status_code(), 401);
    })
    .await;
}
