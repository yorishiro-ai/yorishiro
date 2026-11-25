use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

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
