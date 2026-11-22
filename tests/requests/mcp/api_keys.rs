use axum::http::StatusCode;
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn api_key_tool_issues_tenant_scoped_keys_with_the_same_ceilings() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let administrator = crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let bound = crate::requests::fixtures::issue_api_key(&ctx, tenant.workspace_id, administrator, ApiKeyScope::Migration, false).await;
        let denied = mcp_call(&request, &session, &bound, 2, "tools/call", json!({"name":"create_api_key","arguments":{"name":"from-bound","tenant_scoped":true}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let issuer = yorishiro::models::api_keys::Entity::create_tenant_api_key(&ctx.db, tenant.tenant_id, ApiKeyScope::Migration, Some(administrator), true, "initial-admin").await.unwrap();
        let created = mcp_call(&request, &session, &issuer.plaintext, 3, "tools/call", json!({"name":"create_api_key","arguments":{"name":"tenant-scoped","scope":"write","tenant_scoped":true}})).await;
        let record = tool_result_json(&created);
        assert!(record["workspace_id"].is_null(), "record: {record:?}");
        request.get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", record["full_key"].as_str().unwrap()))
            .await.assert_status(StatusCode::OK);
        assert_eq!(record["scope"], "write");
        assert_eq!(record["effective_scope"], "write");
        let rest = request.post("/api/api-keys").add_header("Authorization", format!("Bearer {}", issuer.plaintext)).json(&json!({"name":"rest-twin","scope":"write","tenant_scoped":true})).await;
        rest.assert_status(StatusCode::CREATED);
        let rest_record = rest.json::<Value>();
        assert!(rest_record["workspace_id"].is_null());
        let revoked = mcp_call(&request, &session, &issuer.plaintext, 4, "tools/call", json!({"name":"revoke_api_key","arguments":{"key_id":record["id"].as_str().unwrap()}})).await;
        assert_eq!(tool_result_json(&revoked)["revoked"], true);
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
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        assert_eq!(before["result"]["isError"], false);
        let keys = request
            .get("/api/api-keys")
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
        yorishiro::models::api_keys::ActiveModel {
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
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let after = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            3,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        assert!(
            after["error"].is_object() || after["result"]["isError"] == true,
            "{after}"
        );
    })
    .await;
}

#[tokio::test]
async fn issued_read_key_reads_but_rejects_valid_mutations_over_both_transports() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator = crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let administrator_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;
        request.post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"template_id":"task-management"})).await.assert_status(StatusCode::CREATED);
        let created = request.post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .add_header("x-workspace-id", tenant.workspace_id.to_string())
            .json(&json!({"name":"read-transport-contract","scope":"read"})).await;
        assert_eq!(created.status_code(), StatusCode::CREATED);
        let key = created.json::<Value>()["full_key"].as_str().unwrap().to_owned();
        request.get("/api/entities").add_query_param("schema_name", "task-management")
            .add_header("Authorization", format!("Bearer {key}"))
            .await.assert_status(StatusCode::OK);
        let session = initialize(&request).await;
        let read = mcp_call(&request, &session, &key, 2, "tools/call",
            json!({"name":"list_entities","arguments":{"schema_name":"task-management"}})).await;
        assert_ne!(read["result"]["isError"], true, "{read}");
        let template = mcp_call(&request, &session, &key, 20, "tools/call",
            json!({"name":"get_template","arguments":{"template_id":"general-notes"}})).await;
        assert_ne!(template["result"]["isError"], true, "{template}");
        let rest = request.get("/api/templates/general-notes")
            .add_header("Authorization", format!("Bearer {key}")) .await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(tool_result_json(&template), rest.json::<Value>());
        let missing = mcp_call(&request, &session, &key, 21, "tools/call",
            json!({"name":"get_template","arguments":{"template_id":"missing"}})).await;
        assert_eq!(missing["result"]["isError"], true, "{missing}");
        let entity = json!({"schema_name":"task-management","entity_type":"task","data":{"title":"denied task"}});
        let schema = json!({"template_id":"general-notes"});
        let migration = json!({"schema_name":"task-management"});
        for (path, body) in [("/api/entities", &entity), ("/api/schemas", &schema), ("/api/migration-jobs/fill-defaults", &migration)] {
            request.post(path).add_header("Authorization", format!("Bearer {key}"))
                .json(body).await.assert_status(StatusCode::FORBIDDEN);
        }
        for (id, name, arguments, scope) in [
            (3, "create_entity", entity, "write"),
            (4, "create_schema", schema, "schema"),
            (5, "fill_defaults", migration, "migration"),
        ] {
            let denied = mcp_call(&request, &session, &key, id, "tools/call", json!({"name":name,"arguments":arguments})).await;
            assert_eq!(denied["result"]["isError"], true, "{denied}");
            assert_eq!(denied["result"]["structuredContent"]["error"]["required_scope"], scope, "{denied}");
        }
    }).await;
}
