use axum::http::StatusCode;
use serde_json::json;
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn service_keys_cannot_act_as_a_current_user() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
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
