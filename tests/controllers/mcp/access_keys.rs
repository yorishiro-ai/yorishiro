use axum::http::StatusCode;
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn access_key_tool_issues_tenant_scoped_keys_with_the_same_ceilings() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let administrator = crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let issuer = yorishiro::models::access_keys::Entity::create_tenant_access_key(&ctx.db, tenant.tenant_id, Some(administrator), "initial-admin").await.unwrap();
        let created = mcp_call(&request, &session, &issuer.plaintext, 3, "tools/call", json!({"name":"create_access_key","arguments":{"name":"tenant-scoped"}})).await;
        let record = tool_result_json(&created);
        assert!(record["workspace_id"].is_null(), "record: {record:?}");
        request.get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", record["access_key"].as_str().unwrap()))
            .await.assert_status(StatusCode::OK);
        let rest = request.post("/api/access-keys").add_header("Authorization", format!("Bearer {}", issuer.plaintext)).json(&json!({"name":"rest-twin"})).await;
        rest.assert_status(StatusCode::CREATED);
        let rest_record = rest.json::<Value>();
        assert!(rest_record["workspace_id"].is_null());
        let updated = mcp_call(&request, &session, &issuer.plaintext, 6, "tools/call", json!({"name":"update_access_key","arguments":{"access_key_id":record["id"].as_str().unwrap(),"name":"renamed","enabled":false}})).await;
        let updated = tool_result_json(&updated);
        assert_eq!(updated["name"], "renamed");
        assert_eq!(updated["state"], "disabled");
        let listed = mcp_call(&request, &session, &issuer.plaintext, 7, "tools/call", json!({"name":"list_access_keys","arguments":{}})).await;
        assert!(tool_result_json(&listed)["keys"].as_array().unwrap().iter().any(|key| key["id"] == record["id"]));
        let deleted = mcp_call(&request, &session, &issuer.plaintext, 4, "tools/call", json!({"name":"delete_access_key","arguments":{"access_key_id":record["id"].as_str().unwrap()}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        let missing = mcp_call(&request, &session, &tenant.api_key, 5, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":Uuid::new_v4()}})).await;
        assert_eq!(missing["result"]["isError"], true);
        assert!(missing.to_string().contains("was not found"));
    }).await;
}

#[tokio::test]
async fn expired_keys_are_rejected_over_rest_and_an_existing_mcp_session() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let session = initialize(&request).await;
        let before = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"get_caller_identity","arguments":{}}),
        )
        .await;
        assert_eq!(before["result"]["isError"], false);
        let keys = request
            .get("/api/access-keys")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .await
            .json::<Value>();
        let prefix = tenant.api_key.split('_').nth(1).unwrap();
        let id = keys["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|key| {
                key["prefix"]
                    .as_str()
                    .is_some_and(|value| value.ends_with(prefix))
            })
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        // Direct write (c): issuance rejects a past expiry, so time passing is injected by moving the stored expiry.
        yorishiro::models::access_keys::ActiveModel {
            id: Set(id),
            expires_at: Set(Some(
                (Utc::now() - chrono::Duration::seconds(1)).fixed_offset(),
            )),
            ..Default::default()
        }
        .update(&ctx.db)
        .await
        .unwrap();
        request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let after = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            3,
            "tools/call",
            json!({"name":"get_caller_identity","arguments":{}}),
        )
        .await;
        assert!(
            after["error"].is_object() || after["result"]["isError"] == true,
            "{after}"
        );
    })
    .await;
}
