use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

#[tokio::test]
async fn mcp_whoami_matches_rest_identity_without_secrets() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            tenant.owner_id,
            ApiKeyScope::Read,
            true,
        )
        .await;
        let session = initialize(&request).await;
        let rest = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &key,
            2,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        let identity = tool_result_json(&result);
        assert_eq!(identity, rest.json::<Value>());
        assert_eq!(identity["scope"], "read");
        assert_eq!(identity["audit"], true);
        assert_eq!(identity["identity_kind"], "user");
        assert!(identity.get("workspace_access").is_none());
        assert_eq!(identity["stored_scope"], "read");
        assert_eq!(identity["bound_workspace_id"], tenant.workspace_id.to_string());
        assert_ne!(identity["workspace_name"].as_str().unwrap(), "");
        assert_ne!(identity["tenant_name"].as_str().unwrap(), "");
        assert!(!result.to_string().contains(&key));
        assert!(!result.to_string().contains("key_hash"));
        let grants = mcp_call(
            &request,
            &session,
            &key,
            3,
            "tools/call",
            json!({"name":"capabilities","arguments":{}}),
        )
        .await;
        let grants = tool_result_json(&grants);
        assert_eq!(grants["scopes"], json!(["read"]));
        assert_eq!(grants["audit"], true);
        assert_eq!(grants["tenant_root"], false);
        let postgres = ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres;
        assert_eq!(grants["constraints"]["database_backend"], if postgres { "postgres" } else { "sqlite" });
        assert_eq!(grants["constraints"]["row_level_security"], postgres);
        assert_eq!(grants["constraints"]["workspace_key_retargeting"], false);
        assert_eq!(grants["constraints"]["tenant_key_target_header"], "X-Workspace-Id");
        assert_eq!(grants["constraints"]["discovery_authorizes_requests"], false);
        let inventory = mcp_call(&request, &session, &key, 4, "tools/list", json!({})).await;
        let tools = grants["tools"].as_array().expect("tool capabilities");
        let inventory = tool_inventory(&inventory);
        assert_eq!(tools.len(), inventory.len());
        for tool in inventory {
            let capability = tools.iter().find(|entry| entry["name"] == tool["name"])
                .expect("every visible tool has a capability");
            assert_eq!(capability["required_scope"], tool["_meta"]["yorishiro/required_scope"]);
            assert_eq!(capability["scope_satisfied"], capability["required_scope"] == "read");
        }
        let denied = mcp_call(&request, &session, &key, 5, "tools/call",
            json!({"name":"create_entity","arguments":{"schema_name":"missing","entity_type":"missing","data":{}}})).await;
        assert_eq!(denied["result"]["isError"], true);
        assert_eq!(denied["result"]["structuredContent"]["error"]["code"], "scope_insufficient");
    })
    .await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one viewer and policy compare login, REST, and MCP discovery while proving a sibling workspace stays private"
)]
async fn viewer_discovery_agrees_across_login_rest_and_mcp() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::workspace_memberships::WorkspaceRole;

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let sibling = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .json(&json!({"name":format!("private-{}", Uuid::new_v4())}))
            .await;
        assert_eq!(sibling.status_code(), StatusCode::CREATED);
        let sibling_id = sibling.json::<Value>()["id"].as_str().unwrap().to_owned();
        let txn = ctx.db.begin().await.unwrap();
        let user = yorishiro::models::tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &crate::requests::fixtures::unique_email("viewer-discovery"),
            "hunter2-hunter2",
            None)
        .await
        .unwrap();
        yorishiro::models::workspace_memberships::Model::grant(
            &txn,
            tenant.workspace_id,
            user.id,
            WorkspaceRole::Viewer,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let policy = request.post("/api/iam/policies")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .json(&json!({"name":"viewer-discovery", "document":{"statements":[{
                "effect":"allow", "actions":["workspace_list"],
                "resources":[{"type":"tenant","tenant_id":tenant.tenant_id}]
            }, {
                "effect":"allow", "actions":["workspace_get"],
                "resources":[{"type":"workspace","tenant_id":tenant.tenant_id,"workspace_id":tenant.workspace_id}]
            }]}})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<Value>()["id"].as_str().unwrap().to_owned();
        request.put(&format!("/api/iam/users/{}/policies/{policy_id}", user.id))
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        let key = issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
        let discovery_key = yorishiro::models::api_keys::Entity::create_tenant_api_key(
            &ctx.db, tenant.tenant_id, ApiKeyScope::Read, Some(user.id), false, "discovery",
        ).await.unwrap().plaintext;
        let session = initialize(&request).await;
        let discovered = mcp_call(
            &request,
            &session,
            &key,
            2,
            "tools/call",
            json!({"name":"discover_workspaces","arguments":{}}),
        )
        .await;
        assert_ne!(discovered["result"]["isError"], true, "{discovered}");
        let candidates = tool_result_json(&discovered);
        assert_eq!(candidates.as_array().unwrap().len(), 1);
        assert_eq!(
            candidates[0]["workspace_id"],
            tenant.workspace_id.to_string()
        );
        let listed = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {discovery_key}"))
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let listed = listed.json::<Value>();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], candidates[0]["workspace_id"]);
        request
            .get(&format!("/api/workspaces/{sibling_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        let login = request
            .post("/auth/root/login")
            .json(&json!({"email":user.email,"password":"hunter2-hunter2"}))
            .await;
        assert_eq!(login.status_code(), StatusCode::OK, "{}", login.text());
        assert_eq!(
            login.json::<Value>()["tenant_id"],
            tenant.tenant_id.to_string()
        );
        let identity = mcp_call(
            &request,
            &session,
            &key,
            3,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        let rest = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let identity = tool_result_json(&identity);
        assert_eq!(identity, rest.json::<Value>());
        assert!(identity.get("workspace_access").is_none());
        assert!(!candidates.to_string().contains(&sibling_id));
    })
    .await;
}
