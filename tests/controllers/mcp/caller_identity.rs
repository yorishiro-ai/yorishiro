use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_access_key};

use super::*;

#[tokio::test]
async fn mcp_caller_identity_matches_rest_without_secrets() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = issue_access_key(&ctx, tenant.workspace_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let rest = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &key,
            2,
            "tools/call",
            json!({"name":"get_caller_identity","arguments":{}}),
        )
        .await;
        let identity = tool_result_json(&result);
        assert_eq!(identity, rest.json::<Value>());
        assert_eq!(identity["identity_kind"], "user");
        assert_eq!(
            identity["yrn"],
            format!("yrn:iam:{}:root", identity["tenant_id"].as_str().unwrap())
        );
        assert!(identity.get("workspace_access").is_none());
        assert_eq!(
            identity["bound_workspace_id"],
            tenant.workspace_id.to_string()
        );
        assert_ne!(identity["workspace_name"].as_str().unwrap(), "");
        assert_ne!(identity["tenant_name"].as_str().unwrap(), "");
        assert!(!result.to_string().contains(&key));
        assert!(!result.to_string().contains("key_hash"));
        assert_eq!(identity["tenant_root"], false);
        let inventory = mcp_call(&request, &session, &key, 3, "tools/list", json!({})).await;
        assert!(
            tool_inventory(&inventory)
                .iter()
                .all(|tool| tool["name"] != "capabilities")
        );
    })
    .await;
}

#[tokio::test]
async fn viewer_discovery_agrees_across_login_rest_and_mcp() {
    use sea_orm::TransactionTrait;

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
        txn.commit().await.unwrap();
        let policy = request.post("/api/policies")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .json(&json!({"name":"viewer-discovery", "document":{"Version":"2012-10-17","Statement":[{
                "Effect":"Allow", "Action":["workspace:ListWorkspaces", "workspace:DiscoverWorkspaces"],
                "Resource":[format!("yrn:tenant:{}", tenant.tenant_id)]
            }, {
                "Effect":"Allow", "Action":["workspace:GetWorkspace"],
                "Resource":[format!("yrn:workspace:{}:{}", tenant.tenant_id, tenant.workspace_id)]
            }]}})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<Value>()["id"].as_str().unwrap().to_owned();
        let key = issue_access_key(&ctx, tenant.workspace_id, user.id).await;
        let discovery_key = yorishiro::models::access_keys::Entity::create_tenant_access_key(
            &ctx.db, tenant.tenant_id, Some(user.id), "discovery",
        ).await.unwrap().plaintext;
        let session = initialize(&request).await;
        let denied = mcp_call(
            &request,
            &session,
            &discovery_key,
            2,
            "tools/call",
            json!({"name":"discover_workspaces","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        let identity = mcp_call(&request, &session, &discovery_key, 3, "tools/call",
            json!({"name":"get_caller_identity","arguments":{}})).await;
        assert_eq!(tool_result_json(&identity)["user_id"], user.id.to_string());
        request.put(&format!("/api/users/{}/policies/{policy_id}", user.id))
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        let discovered = mcp_call(
            &request,
            &session,
            &discovery_key,
            4,
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
        assert!(!candidates.to_string().contains(&sibling_id));
    })
    .await;
}
