use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

#[tokio::test]
async fn workspace_member_tools_match_rest_and_revoke_bound_credentials() {
    use yorishiro::models::{tenant_memberships, user_users, workspace_memberships::WorkspaceRole};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = user_users::create_user(&ctx.db, &crate::requests::fixtures::unique_email("workspace-managed"), "test-password", None).await.unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id).await.unwrap();
        let session = initialize(&request).await;
        let granted = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"grant_workspace_member","arguments":{"workspace_id":tenant.workspace_id,"user_id":user.id,"role":"admin"}})).await;
        assert_eq!(tool_result_json(&granted)["granted"], true);
        let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        crate::requests::fixtures::grant_iam_administrator(&txn, tenant.tenant_id, user.id).await;
        txn.commit().await.unwrap();
        let member_key = issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
        let listed = mcp_call(&request, &session, &member_key, 3, "tools/call", json!({"name":"list_workspace_members","arguments":{"workspace_id":tenant.workspace_id}})).await;
        let rest = request.get(&format!("/api/workspaces/{}/members", tenant.workspace_id)).add_header("Authorization", format!("Bearer {member_key}")).await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&listed), rest.json::<Value>());
        assert_eq!(tool_result_json(&listed), json!([{"user_id":user.id,"role":"admin"}]));
        let tenant_admin = mcp_call(&request, &session, &member_key, 4, "tools/call", json!({"name":"create_api_key","arguments":{"name":"forbidden"}})).await;
        assert_eq!(tenant_admin["result"]["isError"], true);
        crate::requests::fixtures::grant_workspace(&ctx, tenant.workspace_id, user.id, WorkspaceRole::Viewer).await;
        let denied = mcp_call(&request, &session, &member_key, 5, "tools/call", json!({"name":"list_workspace_members","arguments":{"workspace_id":tenant.workspace_id}})).await;
        assert_ne!(denied["result"]["isError"], true);
        let revoked = mcp_call(&request, &session, &tenant.api_key, 7, "tools/call", json!({"name":"revoke_workspace_member","arguments":{"workspace_id":tenant.workspace_id,"user_id":user.id}})).await;
        assert_eq!(tool_result_json(&revoked)["revoked"], true);
        request.get("/api/whoami").add_header("Authorization", format!("Bearer {member_key}")).await.assert_status(StatusCode::UNAUTHORIZED);
        let administrator_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let members = request.get("/api/members").add_header("Authorization", format!("Bearer {administrator_key}")).await.json::<Value>();
        assert!(members.as_array().unwrap().iter().any(|member| member["user_id"] == user.id.to_string()));
    }).await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn workspace_member_tools_reject_cross_tenant_grants() {
    use yorishiro::models::{tenant_memberships, user_users};

    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        crate::requests::require_licence(&ctx);
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = user_users::create_user(&ctx.db, &crate::requests::fixtures::unique_email("workspace-managed"), "test-password", None).await.unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id).await.unwrap();
        let session = initialize(&request).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert_ne!(tenant.tenant_id, other.tenant_id);
        let foreign = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"grant_workspace_member","arguments":{"workspace_id":other.workspace_id,"user_id":user.id,"role":"admin"}})).await;
        assert_eq!(foreign["result"]["isError"], true);
    }).await;
}

#[tokio::test]
async fn mcp_membership_tools_treat_tenant_users_as_tenant_bound_accounts() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let user_name = format!("mcp-member-{}", Uuid::new_v4());
        let auth = format!("Bearer {tenant_admin_key}");
        let created = request
            .post("/api/members/users")
            .add_header("Authorization", &auth)
            .json(&json!({"user_name":user_name,"password":"tenant-user-password"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let user_id = created.json::<Value>()["user_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let session = initialize(&request).await;

        let listed = mcp_call(
            &request,
            &session,
            &tenant_admin_key,
            2,
            "tools/call",
            json!({"name":"list_members","arguments":{}}),
        )
        .await;
        let member = tool_result_json(&listed)
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user_name"] == user_name)
            .cloned()
            .expect("tenant user listed");
        assert!(member["email"].is_null());

        // The tenant user's own key reads its tenant-bound profile.
        let login = request
            .post("/auth/tenant/login")
            .json(&json!({"tenant_id":tenant.tenant_id,"user_name":user_name,"password":"tenant-user-password"}))
            .await;
        login.assert_status_ok();
        let own_key = login.json::<Value>()["api_key"]
            .as_str()
            .unwrap()
            .to_owned();
        let profile = mcp_call(
            &request,
            &session,
            &own_key,
            5,
            "tools/call",
            json!({"name":"get_current_user","arguments":{}}),
        )
        .await;
        let profile = tool_result_json(&profile);
        assert_eq!(profile["user_name"], user_name);
        assert!(profile["email"].is_null());

        // Removing the membership removes the account.
        let removed = mcp_call(
            &request,
            &session,
            &tenant_admin_key,
            6,
            "tools/call",
            json!({"name":"remove_member","arguments":{"user_id":user_id}}),
        )
        .await;
        assert_eq!(tool_result_json(&removed)["removed"], true);
        request
            .post("/auth/tenant/login")
            .json(&json!({"tenant_id":tenant.tenant_id,"user_name":user_name,"password":"tenant-user-password"}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn mcp_tenant_users_cannot_join_another_tenant() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        crate::requests::require_licence(&ctx);
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user_name = format!("stay-home-{}", Uuid::new_v4());
        let tenant_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let auth = format!("Bearer {tenant_key}");
        let session = initialize(&request).await;
        // A tenant user belongs to its own tenant only.
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let created = request
            .post("/api/members/users")
            .add_header("Authorization", &auth)
            .json(&json!({"user_name":user_name,"password":"tenant-user-password"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let home_user = created.json::<Value>()["user_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let other_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, other.tenant_id, other.owner_id)
                .await;
        let joined = mcp_call(
            &request,
            &session,
            &other_key,
            7,
            "tools/call",
            json!({"name":"remove_member","arguments":{"user_id":home_user}}),
        )
        .await;
        assert_eq!(joined["result"]["isError"], false, "{joined}");
        let listed = mcp_call(
            &request,
            &session,
            &tenant_key,
            8,
            "tools/call",
            json!({"name":"list_members","arguments":{}}),
        )
        .await;
        assert!(
            tool_result_json(&listed)
                .as_array()
                .unwrap()
                .iter()
                .any(|member| member["user_id"] == home_user),
            "another tenant cannot remove this tenant's user"
        );
    })
    .await;
}
