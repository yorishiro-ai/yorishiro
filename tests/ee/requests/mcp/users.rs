use axum::http::StatusCode;
use serde_json::{Value, json};
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn current_user_tools_are_self_only_and_secret_free() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let updated = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"update_current_user","arguments":{"display_name":"  Profile  "}}),
        )
        .await;
        let profile = tool_result_json(&updated);
        assert_eq!(profile["id"], tenant.owner_id.to_string());
        assert_eq!(profile["display_name"], "Profile");
        assert!(profile.get("password_hash").is_none());
        assert!(profile.get("oauth_subject_id").is_none());
        let rest = request
            .get("/api/users/me")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        rest.assert_status_ok();
        assert_eq!(profile, rest.json::<Value>());
        let invalid = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            3,
            "tools/call",
            json!({"name":"update_current_user","arguments":{"display_name":" "}}),
        )
        .await;
        assert_eq!(invalid["result"]["isError"], true);
        request
            .put("/api/users/me")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"display_name":"Other", "email":"replacement@example.com"}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        request
            .put("/api/users/me")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"display_name":null}))
            .await
            .assert_status_ok();
        let current = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            4,
            "tools/call",
            json!({"name":"get_current_user","arguments":{}}),
        )
        .await;
        assert!(tool_result_json(&current)["display_name"].is_null());
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap()
        .plaintext;
        let denied = mcp_call(
            &request,
            &session,
            &service,
            5,
            "tools/call",
            json!({"name":"get_current_user","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        let denied_update = mcp_call(
            &request,
            &session,
            &service,
            6,
            "tools/call",
            json!({"name":"update_current_user","arguments":{"display_name":"Service"}}),
        )
        .await;
        assert_eq!(denied_update["result"]["isError"], true);
        request
            .get("/api/users/me")
            .add_header("Authorization", format!("Bearer {service}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    })
    .await;
}
