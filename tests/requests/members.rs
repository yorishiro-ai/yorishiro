use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant, issue_api_key};
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;

#[tokio::test]
async fn owner_can_list_and_add_members() {
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let args = TenantArgs::default();
        let owner_email = args.owner_email.clone();
        let TestTenant {
            api_key: owner_key, ..
        } = TestTenant::create(&request, &ctx, args).await;

        // The invitee must already have an account before they can be added by email.
        let invitee = yorishiro::models::user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("invitee"),
            "hunter2-hunter2",
            None,
        )
        .await
        .expect("create invitee");

        let response = request
            .post("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "email": invitee.email,
                "role": "member",
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::CREATED);
        let body: serde_json::Value = response.json();
        assert_eq!(body["user_id"], invitee.id.to_string());
        assert_eq!(body["role"], "member");

        let response = request
            .get("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        let emails: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["email"].as_str().unwrap())
            .collect();
        assert!(emails.contains(&owner_email.as_str()));
        assert!(emails.contains(&invitee.email.as_str()));
    })
    .await;
}

#[tokio::test]
async fn add_member_rejects_an_email_with_no_account() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            api_key: owner_key, ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .post("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "email": super::fixtures::unique_email("nobody"),
                "role": "member",
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn member_role_cannot_manage_members() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let member = yorishiro::models::user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("member"),
            "hunter2-hunter2",
            None,
        )
        .await
        .expect("create member");
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant_id,
            member.id,
            MembershipRole::Member,
        )
        .await
        .expect("add member");
        let member_key =
            issue_api_key(&ctx, workspace_id, member.id, ApiKeyScope::Migration, false).await;

        let response = request
            .get("/api/members")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
async fn members_endpoints_require_authentication() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/members").await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    })
    .await;
}
