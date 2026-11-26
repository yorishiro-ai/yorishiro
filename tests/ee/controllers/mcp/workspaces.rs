use axum::http::{HeaderValue, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::schema_schemas;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_access_key};

use super::*;

#[tokio::test]
async fn workspace_deletion_reports_fork_references_without_exposing_destination() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ModelTrait, TransactionTrait};
    use yorishiro::models::{_entities::workspace_schema_forks, tenancy};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"workspace-fork", "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let txn = ctx.db.begin().await.unwrap();
        let destination = tenancy::create_workspace(&txn, tenant.tenant_id,
            &format!("workspace-fork-{}", Uuid::now_v7()), None, None).await.unwrap();
        let source = schema_schemas::create_schema(&txn, tenant.tenant_id, tenant.workspace_id,
            serde_json::from_value(definition.clone()).unwrap(), None, None).await.unwrap().0;
        let target = schema_schemas::create_schema(&txn, tenant.tenant_id, destination.id,
            serde_json::from_value(definition).unwrap(), None, None).await.unwrap().0;
        // Direct write (a): no operation creates a fork onto a schema the test names; the fork operation makes its own, and fails on PostgreSQL inside this transaction.
        let fork = workspace_schema_forks::ActiveModel {
            id: Set(Uuid::now_v7()), tenant_id: Set(tenant.tenant_id), workspace_id: Set(destination.id),
            source_workspace_id: Set(tenant.workspace_id), source_schema_id: Set(source.id),
            source_schema_version: Set(1), source_schema_name: Set("workspace-fork".into()),
            fork_schema_id: Set(target.id), customized: Set(false), ..Default::default()
        }.insert(&txn).await.unwrap();
        txn.commit().await.unwrap();
        let denied = mcp_call(&request, &session, &tenant_admin_key, 2, "tools/call",
            json!({"name":"delete_workspace","arguments":{"workspace_id":tenant.workspace_id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        assert!(denied.to_string().contains("schema forks"), "{denied}");
        assert!(!denied.to_string().contains(&destination.id.to_string()));
        request.get(&format!("/api/workspaces/{}", tenant.workspace_id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status_ok();
        fork.delete(&ctx.db).await.unwrap();
        let destination_key = issue_access_key(&ctx, destination.id, tenant.owner_id).await;
        let deleted = mcp_call(&request, &session, &tenant_admin_key, 3, "tools/call",
            json!({"name":"delete_workspace","arguments":{"workspace_id":tenant.workspace_id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        request.get(&format!("/api/workspaces/{}", tenant.workspace_id))
            .add_header("Authorization", format!("Bearer {destination_key}")).await.assert_status(StatusCode::NOT_FOUND);
    }).await;
}

#[tokio::test]
async fn workspace_tools_share_rest_details_updates_and_binding_limits() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        for header in ["x-tenant-id", "x-workspace-id"] {
            let invalid = HeaderValue::from_bytes(b"\xff").unwrap();
            let rest = request.get("/api/caller-identity")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .add_header(header, invalid.clone()).await;
            assert_eq!(rest.status_code(), StatusCode::UNPROCESSABLE_ENTITY);

            #[cfg(feature = "enterprise")]
            request.get("/api/schemas/upstream-changes")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .add_header(header, invalid.clone()).await
                .assert_status(StatusCode::UNPROCESSABLE_ENTITY);

            for (id, tool) in [(2, "get_caller_identity"), (3, "list_schemas"), (4, "list_workspace_audit_events")] {
                let response = request.post("/mcp")
                    .add_header("Accept", "application/json, text/event-stream")
                    .add_header("MCP-Session-Id", session.clone())
                    .add_header("MCP-Protocol-Version", "2025-03-26")
                    .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                    .add_header(header, invalid.clone())
                    .json(&json!({"jsonrpc":"2.0", "id":id, "method":"tools/call", "params":{"name":tool,"arguments":{}}}))
                    .await;
                let body = rpc_body(&response.text());
                assert_eq!(body["error"]["code"], -32602, "{body}");
                assert!(body["error"]["message"].as_str().unwrap().contains(header));
            }
        }
        let tenant_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let missing = mcp_call(&request, &session, &tenant_key, 20, "tools/call", json!({"name":"list_schemas","arguments":{}})).await;
        assert_ne!(missing["result"]["isError"], true);
        let tenant_detail = mcp_call(&request, &session, &tenant_key, 21, "tools/call", json!({"name":"get_tenant","arguments":{}})).await;
        assert_ne!(tenant_detail["result"]["isError"], true);
        let detail = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"get_workspace","arguments":{"workspace_id":tenant.workspace_id}})).await;
        let rest = request.get(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {}",tenant.api_key)).await;
        assert_eq!(tool_result_json(&detail),rest.json::<Value>());
        let denied_quota = mcp_call(&request, &session, &tenant.api_key, 22, "tools/call", json!({"name":"update_workspace","arguments":{"workspace_id":tenant.workspace_id,"replace_max_entities":true,"max_entities":42}})).await;
        assert_eq!(denied_quota["result"]["isError"], true);
        let updated = mcp_call(&request, &session, &tenant_key, 3, "tools/call", json!({"name":"update_workspace","arguments":{"workspace_id":tenant.workspace_id,"name":"renamed","replace_max_entities":true,"max_entities":42}})).await;
        assert_eq!(tool_result_json(&updated)["name"],"renamed");
        let rest = request.get(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {}",tenant.api_key)).await;
        assert_eq!(rest.json::<Value>()["max_entities"],42);
        let rest = request.put(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {tenant_key}")).json(&json!({"replace_max_entities":true,"max_entities":null})).await;
        rest.assert_status_ok();
        assert!(rest.json::<Value>()["max_entities"].is_null());
        let invalid = request.put(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {tenant_key}")).json(&json!({"name":"must-not-save","replace_max_entities":true,"max_entities":-1})).await;
        assert!(!invalid.status_code().is_success());
        let created = mcp_call(&request, &session, &tenant_key, 4, "tools/call", json!({"name":"create_workspace","arguments":{"name":"second"}})).await;
        let second = tool_result_json(&created)["id"].as_str().unwrap().to_owned();
        let targeted = mcp_call(&request, &session, &tenant_key, 9, "tools/call", json!({"name":"update_workspace","arguments":{"workspace_id":second,"name":"targeted"}})).await;
        assert_eq!(tool_result_json(&targeted)["name"], "targeted");
        let deleted = mcp_call(&request, &session, &tenant_key, 10, "tools/call", json!({"name":"delete_workspace","arguments":{"workspace_id":second}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"],true);
    }).await;
}

#[tokio::test]
async fn mcp_workspace_discovery_respects_credential_tenant_and_service_identity() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let session = initialize(&request).await;
        let response = mcp_call(
            &request,
            &session,
            &tenant_key,
            2,
            "tools/call",
            json!({"name":"discover_workspaces","arguments":{}}),
        )
        .await;
        let workspaces = tool_result_json(&response);
        let candidates = workspaces.as_array().unwrap();
        assert!(
            candidates
                .iter()
                .any(|workspace| workspace["workspace_id"] == tenant.workspace_id.to_string())
        );
        assert!(
            candidates
                .iter()
                .all(|workspace| workspace["tenant_id"] == tenant.tenant_id.to_string())
        );
        let service = crate::requests::fixtures::create_key(&ctx.db, tenant.workspace_id, None)
            .await
            .unwrap();
        let response = mcp_call(
            &request,
            &session,
            &service.plaintext,
            3,
            "tools/call",
            json!({"name":"discover_workspaces","arguments":{}}),
        )
        .await;
        assert_eq!(response["result"]["isError"], true);
        assert!(!response.to_string().contains(&service.plaintext));
        let rest = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", service.plaintext))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &service.plaintext,
            5,
            "tools/call",
            json!({"name":"get_caller_identity","arguments":{}}),
        )
        .await;
        let identity = tool_result_json(&result);
        assert_eq!(identity, rest.json::<Value>());
        assert_eq!(identity["tenant_root"], false);
        assert_eq!(identity["identity_kind"], "service");
        assert!(identity.get("workspace_access").is_none());
        assert!(identity["user_id"].is_null());
        assert!(!result.to_string().contains(&service.plaintext));
    })
    .await;
}
