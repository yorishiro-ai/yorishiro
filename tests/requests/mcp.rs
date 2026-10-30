use axum::http::StatusCode;
use chrono::Utc;
use loco_rs::TestServer;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::{entity_entities, schema_schemas};

use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant, issue_api_key};
use crate::EnvGuard;

pub(super) fn rpc_body(text: &str) -> Value {
    let payload = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .find(|line| !line.is_empty())
        .unwrap_or(text.trim());
    serde_json::from_str(payload).unwrap_or_else(|err| panic!("invalid MCP response {err}: {text}"))
}

pub(super) async fn initialize(request: &TestServer) -> String {
    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "yorishiro-test", "version": "1.0" }
            }
        }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "response: {}",
        response.text()
    );
    let _ = rpc_body(&response.text());
    let session = response
        .headers()
        .get("mcp-session-id")
        .expect("MCP session id")
        .to_str()
        .expect("MCP session id is text")
        .to_owned();

    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .add_header("MCP-Session-Id", session.clone())
        .add_header("MCP-Protocol-Version", "2025-03-26")
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::ACCEPTED,
        "response: {}",
        response.text()
    );
    session
}

async fn mcp_call(
    request: &TestServer,
    session: &str,
    api_key: &str,
    id: i64,
    method: &str,
    params: Value,
) -> Value {
    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .add_header("MCP-Session-Id", session)
        .add_header("MCP-Protocol-Version", "2025-03-26")
        .add_header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "response: {}",
        response.text()
    );
    rpc_body(&response.text())
}

fn tool_inventory(response: &Value) -> Vec<Value> {
    response["result"]["tools"]
        .as_array()
        .expect("tools/list result")
        .clone()
}

fn tool_result_json(response: &Value) -> Value {
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool result text"),
    )
    .expect("JSON tool result")
}

#[tokio::test]
async fn expired_keys_are_rejected_over_rest_and_an_existing_mcp_session() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
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
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
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
        request.post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"template_id":"task-management"})).await.assert_status(StatusCode::CREATED);
        let created = request.post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
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
        assert_eq!(identity["workspace_access"]["source"], "tenant_role");
        assert_eq!(identity["workspace_access"]["workspace_admin"], true);
        assert_eq!(identity["stored_scope"], "read");
        assert_eq!(identity["bound_workspace_id"], tenant.workspace_id.to_string());
        assert!(!identity["workspace_name"].as_str().unwrap().is_empty());
        assert!(!identity["tenant_name"].as_str().unwrap().is_empty());
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
        assert_eq!(grants["tenant_admin"], true);
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
        for tool in &inventory {
            let capability = tools.iter().find(|entry| entry["name"] == tool["name"])
                .expect("every visible tool has a capability");
            assert_eq!(capability["required_scope"], tool["_meta"]["yorishiro/required_scope"]);
            assert_eq!(capability["scope_satisfied"], capability["required_scope"] == "read");
        }
        let denied = mcp_call(&request, &session, &key, 5, "tools/call",
            json!({"name":"create_entity","arguments":{"schema_name":"missing","entity_type":"missing","data":{}}})).await;
        let create = tools.iter().find(|entry| entry["name"] == "create_entity").unwrap();
        assert_eq!(create["required_scope"], denied["result"]["structuredContent"]["error"]["required_scope"]);
    })
    .await;
}

#[tokio::test]
async fn viewer_discovery_agrees_across_login_rest_and_mcp() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::tenant_memberships::MembershipRole;
    use yorishiro::models::workspace_memberships::WorkspaceRole;

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let sibling = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"name":format!("private-{}", uuid::Uuid::new_v4())}))
            .await;
        assert_eq!(sibling.status_code(), StatusCode::CREATED);
        let sibling_id = sibling.json::<Value>()["id"].as_str().unwrap().to_owned();
        let txn = ctx.db.begin().await.unwrap();
        let user = yorishiro::models::tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &super::fixtures::unique_email("viewer-discovery"),
            "hunter2-hunter2",
            None,
            MembershipRole::Viewer,
        )
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
        let key = issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
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
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let listed = listed.json::<Value>();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], candidates[0]["workspace_id"]);
        request
            .get(&format!("/api/workspaces/{sibling_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
        let login = request
            .post("/auth/login")
            .json(&json!({"email":user.email,"password":"hunter2-hunter2"}))
            .await;
        assert_eq!(login.status_code(), StatusCode::OK, "{}", login.text());
        assert_eq!(
            login.json::<Value>()["workspace_id"],
            candidates[0]["workspace_id"]
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
        assert_eq!(
            identity["workspace_access"]["source"],
            "workspace_membership"
        );
        assert_eq!(identity["workspace_access"]["workspace_admin"], false);
        assert!(!candidates.to_string().contains(&sibling_id));
    })
    .await;
}

#[tokio::test]
async fn mcp_workspace_discovery_respects_credential_tenant_and_service_identity() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let response = mcp_call(
            &request,
            &session,
            &tenant.api_key,
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
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
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
        let workspaces = tool_result_json(&response);
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(
            workspaces[0]["workspace_id"],
            tenant.workspace_id.to_string()
        );
        assert!(!response.to_string().contains(&service.plaintext));
        let grants = mcp_call(
            &request,
            &session,
            &service.plaintext,
            4,
            "tools/call",
            json!({"name":"capabilities","arguments":{}}),
        )
        .await;
        let grants = tool_result_json(&grants);
        assert_eq!(grants["scopes"], json!(["read"]));
        assert_eq!(grants["audit"], false);
        assert_eq!(grants["tenant_admin"], false);
        let rest = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", service.plaintext))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &service.plaintext,
            5,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        let identity = tool_result_json(&result);
        assert_eq!(identity, rest.json::<Value>());
        assert_eq!(identity["identity_kind"], "service");
        assert_eq!(identity["workspace_access"]["source"], "service_key");
        assert_eq!(identity["workspace_access"]["workspace_admin"], false);
        assert!(identity["user_id"].is_null());
        assert_eq!(identity["stored_scope"], "read");
        assert_eq!(identity["scope"], "read");
        assert_eq!(
            identity["bound_workspace_id"],
            tenant.workspace_id.to_string()
        );
        assert!(!result.to_string().contains(&service.plaintext));
    })
    .await;
}

#[tokio::test]
async fn mcp_identity_distinguishes_stored_and_membership_capped_scope() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::tenant_memberships::{self, MembershipRole};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        request.post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"template_id":"general-notes"})).await.assert_status(StatusCode::CREATED);
        let txn = ctx.db.begin().await.unwrap();
        let member = yorishiro::models::tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &super::fixtures::unique_email("identity-member"),
            "hunter2-hunter2",
            None,
            MembershipRole::Admin,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            member.id,
            ApiKeyScope::Migration,
            false,
        )
        .await;
        let session = initialize(&request).await;
        for (id, role, expected) in [
            (2, MembershipRole::Viewer, "read"),
            (4, MembershipRole::Admin, "migration"),
        ] {
            super::fixtures::grant_workspace(
                &ctx,
                tenant.workspace_id,
                member.id,
                yorishiro::models::workspace_memberships::WorkspaceRole::Admin,
            )
            .await;
            tenant_memberships::add_member(&ctx.db, tenant.tenant_id, member.id, role)
                .await
                .unwrap();
            let rest = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(rest.status_code(), StatusCode::OK);
            let result = mcp_call(
                &request,
                &session,
                &key,
                id,
                "tools/call",
                json!({"name":"whoami","arguments":{}}),
            )
            .await;
            let identity = tool_result_json(&result);
            assert_eq!(identity, rest.json::<Value>());
            assert_eq!(identity["stored_scope"], "migration");
            assert_eq!(identity["scope"], expected);
            assert_eq!(identity["identity_kind"], "user");
            if role == MembershipRole::Viewer {
                let definition = json!({
                    "name":"demoted-key-must-not-create",
                    "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}
                });
                request.post("/api/schemas")
                    .add_header("Authorization", format!("Bearer {key}"))
                    .json(&definition).await.assert_status(StatusCode::FORBIDDEN);
                let denied = mcp_call(&request, &session, &key, 20, "tools/call",
                    json!({"name":"create_schema","arguments":{"definition":definition}})).await;
                assert_eq!(denied["result"]["isError"], true, "{denied}");
                assert_eq!(denied["result"]["structuredContent"]["error"]["required_scope"], "schema");
                let entity = json!({"schema_name":"general-notes","entity_type":"note","data":{"title":"denied"}});
                request.post("/api/entities")
                    .add_header("Authorization", format!("Bearer {key}"))
                    .json(&entity).await.assert_status(StatusCode::FORBIDDEN);
                let denied = mcp_call(&request, &session, &key, 21, "tools/call",
                    json!({"name":"create_entity","arguments":entity})).await;
                assert_eq!(denied["result"]["isError"], true, "{denied}");
                assert_eq!(denied["result"]["structuredContent"]["error"]["required_scope"], "write");
            }
            assert_eq!(
                identity["bound_workspace_id"],
                tenant.workspace_id.to_string()
            );
            assert!(!result.to_string().contains(&key));
            let grants = mcp_call(
                &request,
                &session,
                &key,
                id + 1,
                "tools/call",
                json!({"name":"capabilities","arguments":{}}),
            )
            .await;
            let grants = tool_result_json(&grants);
            assert_eq!(grants["tenant_admin"], role == MembershipRole::Admin);
            assert_eq!(grants["workspace_access"], identity["workspace_access"]);
            assert_eq!(
                identity["workspace_access"]["source"],
                if role == MembershipRole::Admin {
                    "tenant_role"
                } else {
                    "workspace_membership"
                }
            );
            assert_eq!(identity["workspace_access"]["workspace_admin"], true);
            assert_eq!(
                grants["scopes"].as_array().unwrap().len(),
                if role == MembershipRole::Admin { 4 } else { 1 }
            );
        }
    })
    .await;
}

#[tokio::test]
async fn mcp_authentication_database_failure_is_redacted() {
    if !crate::require_sqlite_backend() {
        return;
    }
    use sea_orm::ConnectionTrait;

    boot_request::<App, _, _>(|request, ctx| async move {
        let session = initialize(&request).await;
        ctx.db
            .execute_unprepared("ALTER TABLE api_keys RENAME TO private_auth_diagnostic")
            .await
            .expect("break authentication lookup in this isolated test database");
        let response = mcp_call(
            &request,
            &session,
            "not-a-real-key",
            2,
            "tools/call",
            json!({"name": "list_entities", "arguments": {}}),
        )
        .await;
        assert_eq!(response["error"]["message"], "internal server error");
        assert!(!response.to_string().contains("api_keys"));
        assert!(!response.to_string().contains("private_auth_diagnostic"));
    })
    .await;
}

#[tokio::test]
async fn mcp_authenticator_permission_denials_are_structured_not_internal_errors() {
    use std::sync::Arc;
    use yorishiro::controllers::middleware::auth::Authenticator;

    struct PermissionDenied;
    #[loco_rs::prelude::async_trait]
    impl Authenticator for PermissionDenied {
        async fn authenticate(
            &self,
            _app: &loco_rs::app::AppContext,
            _key: &str,
            _headers: &[(String, String)],
        ) -> Result<yorishiro::models::api_keys::AuthContext, yorishiro::YorishiroError> {
            Err(yorishiro::YorishiroError::ScopeInsufficient {
                message: "workspace access denied".into(),
                hint: "request a workspace grant".into(),
            })
        }
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let session = initialize(&request).await;
        ctx.shared_store
            .insert(Arc::new(PermissionDenied) as Arc<dyn Authenticator>);
        for (id, name) in [(2, "list_entities"), (3, "whoami")] {
            let result = mcp_call(
                &request,
                &session,
                "denied-key",
                id,
                "tools/call",
                json!({"name":name,"arguments":{}}),
            )
            .await;
            assert_eq!(result["result"]["isError"], true, "{result}");
            let error = &result["result"]["structuredContent"]["error"];
            assert_eq!(error["code"], "scope_insufficient");
            assert_eq!(error["message"], "workspace access denied");
            assert_eq!(error["hint"], "request a workspace grant");
            assert!(error.get("required_scope").is_none());
            assert!(error.get("effective_scope").is_none());
        }
    })
    .await;
}

/// Transport compatibility regressions from the MCP HTTP issues.
#[tokio::test]
async fn mcp_transport_rejects_unconfigured_host_by_default() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request
            .post("/mcp")
            .add_header("Host", "mcp.remote.test")
            .add_header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
        assert!(response.text().contains("Host header is not allowed"));
    })
    .await;
}

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn mcp_transport_requires_protocol_accept_headers() {
    let guard = EnvGuard::capture(&[crate::env::TestEnv::McpAllowedHosts.name()]);
    guard.set(
        crate::env::TestEnv::McpAllowedHosts.name(),
        "mcp.remote.test",
    );
    boot_request::<App, _, _>(|request, _ctx| async move {
        let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        for accept in [None, Some("*/*"), Some("application/json")] {
            let mut call = request.post("/mcp").add_header("Host", "mcp.remote.test");
            if let Some(accept) = accept {
                call = call.add_header("Accept", accept);
            }
            let response = call.json(&body).await;
            assert_ne!(response.status_code(), StatusCode::FORBIDDEN);
            assert_eq!(response.status_code(), StatusCode::NOT_ACCEPTABLE);
        }
    })
    .await;
}

/// A server instance is retained by rmcp for the whole session.
/// Origin operations remain ungated, like their REST counterparts, in an existing session.
#[tokio::test]
async fn mcp_origin_tools_match_ungated_rest_when_same_session_licence_state_is_missing() {
    boot_request::<App, _, _>(|request, ctx| async move {
        assert!(
            ctx.shared_store
                .get::<std::sync::Arc<LicenceState>>()
                .is_some_and(|state| state.is_active()),
            "this case requires a valid YORISHIRO_LICENSE_KEY at application boot"
        );
        let TestTenant {
            workspace_id,
            owner_id,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Schema, false).await;
        let session = initialize(&request).await;

        let licensed = mcp_call(&request, &session, &key, 2, "tools/list", json!({})).await;
        let licensed_tools = tool_inventory(&licensed);
        for name in ["list_upstream_changes", "merge_preview", "merge_apply"] {
            assert!(licensed_tools.iter().any(|tool| tool["name"] == name));
        }
        for tool in &licensed_tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
        }

        // Inject missing process state without manufacturing a licence or replacing its verifier.
        assert!(
            ctx.shared_store
                .remove::<std::sync::Arc<LicenceState>>()
                .is_some()
        );
        let community = mcp_call(&request, &session, &key, 3, "tools/list", json!({})).await;
        let community_tools = tool_inventory(&community);
        let grants = mcp_call(
            &request,
            &session,
            &key,
            4,
            "tools/call",
            json!({"name":"capabilities","arguments":{}}),
        )
        .await;
        let grants = tool_result_json(&grants);
        let capabilities = grants["tools"]
            .as_array()
            .expect("visible tool capabilities");
        assert_eq!(capabilities.len(), community_tools.len());
        for name in ["list_upstream_changes", "merge_preview", "merge_apply"] {
            assert!(community_tools.iter().any(|tool| tool["name"] == name));
            assert!(capabilities.iter().any(|tool| tool["name"] == name));
        }
        let rest = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        rest.assert_status(StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &key,
            10,
            "tools/call",
            json!({"name":"list_upstream_changes","arguments":{}}),
        )
        .await;
        assert_ne!(result["result"]["isError"], true, "{result}");
        assert_eq!(tool_result_json(&result), rest.json::<Value>());
    })
    .await;
}

/// Origin tools read templates through the control-plane connection, keep schema work on an
/// RLS-scoped transaction, enforce read/schema scopes, and explicitly commit a successful merge.
#[tokio::test]
async fn origin_mcp_tools_enforce_scope_and_commit_the_merge() {
    boot_request::<App, _, _>(|request, ctx| async move {
        assert!(
            ctx.shared_store
                .get::<std::sync::Arc<LicenceState>>()
                .expect("production boot installs licence state")
                .is_active(),
            "licensed MCP merge tests require a valid YORISHIRO_LICENSE_KEY at boot"
        );
        let TestTenant {
            workspace_id,
            owner_id,
            api_key: schema_key,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;

        let original = json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        });
        let template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {schema_key}"))
            .json(&json!({ "name": "library-note", "definition": original }))
            .await;
        assert_eq!(
            template.status_code(),
            StatusCode::CREATED,
            "{}",
            template.text()
        );
        let template_id: Uuid = template.json::<Value>()["id"]
            .as_str()
            .expect("template id")
            .parse()
            .expect("template UUID");

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {schema_key}"))
            .json(&json!({ "template_id": template_id.to_string() }))
            .await;
        assert_eq!(
            create.status_code(),
            StatusCode::CREATED,
            "response: {}",
            create.text()
        );
        let schema_id: Uuid = create.json::<Value>()["schema"]["id"]
            .as_str()
            .expect("schema id")
            .parse()
            .expect("schema UUID");

        let updated = json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "category": { "type": "string" }
                } }
            }
        });
        let update = request
            .put(&format!("/api/template-library/{template_id}"))
            .add_header("Authorization", format!("Bearer {schema_key}"))
            .json(&json!({ "definition": updated }))
            .await;
        assert_eq!(update.status_code(), StatusCode::OK, "{}", update.text());

        let session = initialize(&request).await;
        let changes = mcp_call(
            &request,
            &session,
            &read_key,
            2,
            "tools/call",
            json!({
                "name": "list_upstream_changes",
                "arguments": { "limit": 10, "offset": 0 }
            }),
        )
        .await;
        assert_eq!(changes["result"]["isError"], false, "MCP call: {changes}");
        let changes = tool_result_json(&changes);
        assert_eq!(changes.as_array().expect("change list").len(), 1);

        let preview = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({
                "name": "merge_preview",
                "arguments": { "schema_id": schema_id }
            }),
        )
        .await;
        assert_eq!(preview["result"]["isError"], false, "MCP call: {preview}");
        assert_eq!(tool_result_json(&preview)["summary"]["auto_add"], 1);

        let denied = mcp_call(
            &request,
            &session,
            &read_key,
            4,
            "tools/call",
            json!({
                "name": "merge_apply",
                "arguments": { "schema_id": schema_id }
            }),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true, "MCP call: {denied}");
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["code"],
            "scope_insufficient"
        );
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["effective_scope"],
            "read"
        );
        assert_eq!(
            schema_schemas::get_active_schema(&ctx.db, workspace_id, "library-note")
                .await
                .expect("active schema after denial")
                .version,
            1
        );

        let merged = mcp_call(
            &request,
            &session,
            &schema_key,
            5,
            "tools/call",
            json!({
                "name": "merge_apply",
                "arguments": { "schema_id": schema_id }
            }),
        )
        .await;
        assert_eq!(merged["result"]["isError"], false, "MCP call: {merged}");
        assert_eq!(tool_result_json(&merged)["schema"]["version"], 2);

        let persisted = schema_schemas::get_active_schema(&ctx.db, workspace_id, "library-note")
            .await
            .expect("persisted merged schema");
        assert_eq!(persisted.version, 2);
        assert!(
            persisted.definition.entity_types["note"]
                .fields
                .contains_key("category")
        );
    })
    .await;
}

/// The fill_defaults MCP path must be discoverable, execute with Migration scope, persist its
/// writes and snapshot, and leave an audit record. A lower-scope key receives an isError result
/// and cannot change the entity.
#[tokio::test]
async fn fill_defaults_mcp_executes_and_enforces_migration_scope() {
    use sea_orm::EntityTrait;
    super::fill_defaults::boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id,
            api_key: migration_key,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Migration,
                ..Default::default()
            },
        )
        .await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;
        let audit_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, true).await;

        let v1 = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        }))
        .expect("parse v1 definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, v1, None, None)
            .await
            .expect("create v1 schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: json!({ "title": "before" }),
            },
            None,
        )
        .await
        .expect("create entity on v1");
        let v2 = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "priority": { "type": "integer", "required": true, "default": 7 }
                } }
            }
        }))
        .expect("parse v2 definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, v2, None, None)
            .await
            .expect("create v2 schema");

        let session = initialize(&request).await;
        let tools = mcp_call(&request, &session, &read_key, 2, "tools/list", json!({})).await;
        assert!(
            tools["result"]["tools"]
                .as_array()
                .expect("tools list")
                .iter()
                .any(|tool| tool["name"] == "fill_defaults")
        );

        let denied = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({ "name": "fill_defaults", "arguments": { "schema_name": "note" } }),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["required_scope"],
            "migration"
        );
        assert!(
            denied["result"]["content"][0]["text"]
                .as_str()
                .expect("denial text")
                .contains("scope")
        );
        let unchanged = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read denied entity");
        assert_eq!(unchanged.data, json!({ "title": "before" }));

        let call = mcp_call(
            &request,
            &session,
            &migration_key,
            4,
            "tools/call",
            json!({ "name": "fill_defaults", "arguments": { "schema_name": "note" } }),
        )
        .await;
        assert_eq!(call["result"]["isError"], false, "MCP call: {call}");
        let report: Value = serde_json::from_str(
            call["result"]["content"][0]["text"]
                .as_str()
                .expect("report text"),
        )
        .expect("serialized fill_defaults report");
        assert_eq!(report["schema_name"], "note");
        assert!(report["job_id"].as_str().is_some());
        assert!(report.get("entities_updated").is_none());
        let job_id: Uuid = report["job_id"].as_str().unwrap().parse().unwrap();
        let pending = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .unwrap();
        assert_eq!(pending.data, json!({"title": "before"}));
        super::fill_defaults::finish_via_api(
            &request,
            &ctx,
            job_id,
            &migration_key,
            &format!("/api/migration-jobs/{job_id}"),
        )
        .await;

        let updated = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read filled entity");
        assert_eq!(updated.data, json!({ "title": "before", "priority": 7 }));
        assert_eq!(updated.schema_version, 1);
        let job_id: Uuid = report["job_id"].as_str().unwrap().parse().unwrap();

        for (index, name) in ["migration_job_status", "undo_migration_job"]
            .iter()
            .enumerate()
        {
            let denied = mcp_call(
                &request,
                &session,
                &read_key,
                10 + index as i64,
                "tools/call",
                json!({"name":name,"arguments":{"job_id":job_id}}),
            )
            .await;
            assert_eq!(denied["result"]["isError"], true);
            assert_eq!(
                denied["result"]["structuredContent"]["error"]["required_scope"],
                "migration"
            );
        }
        let status = mcp_call(
            &request,
            &session,
            &migration_key,
            12,
            "tools/call",
            json!({"name":"migration_job_status","arguments":{"job_id":job_id}}),
        )
        .await;
        assert_eq!(status["result"]["isError"], false);
        let status: Value =
            serde_json::from_str(status["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let rest = request
            .get(&format!("/api/migration-jobs/{job_id}"))
            .add_header("Authorization", format!("Bearer {migration_key}"))
            .await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(status, rest.json::<Value>());
        let missing = mcp_call(
            &request,
            &session,
            &migration_key,
            13,
            "tools/call",
            json!({"name":"migration_job_status","arguments":{"job_id":Uuid::new_v4()}}),
        )
        .await;
        assert_eq!(missing["result"]["isError"], true);
        let undo = mcp_call(
            &request,
            &session,
            &migration_key,
            14,
            "tools/call",
            json!({"name":"undo_migration_job","arguments":{"job_id":job_id}}),
        )
        .await;
        assert_eq!(undo["result"]["isError"], false, "{undo}");
        let undo: Value =
            serde_json::from_str(undo["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(undo["snapshot_job_id"], job_id.to_string());
        let undo_id = undo["job_id"].as_str().unwrap().parse().unwrap();
        super::fill_defaults::finish_via_api(
            &request,
            &ctx,
            undo_id,
            &migration_key,
            &format!("/api/migration-jobs/{undo_id}"),
        )
        .await;
        let restored = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read undone entity");
        assert_eq!(restored.data, json!({ "title": "before" }));
        assert_eq!(restored.schema_version, 1);

        let denied = mcp_call(
            &request,
            &session,
            &read_key,
            15,
            "tools/call",
            json!({"name":"reindex_workspace","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["required_scope"],
            "migration"
        );
        let reindex = mcp_call(
            &request,
            &session,
            &migration_key,
            16,
            "tools/call",
            json!({"name":"reindex_workspace","arguments":{}}),
        )
        .await;
        assert_eq!(reindex["result"]["isError"], false, "{reindex}");
        let reindex = tool_result_json(&reindex);
        let reindex_id: Uuid = reindex["job_id"].as_str().unwrap().parse().unwrap();
        let outbox =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(reindex_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("committed reindex dispatch");
        assert_eq!(outbox.workspace_id, Some(workspace_id));
        let status = mcp_call(
            &request,
            &session,
            &migration_key,
            17,
            "tools/call",
            json!({"name":"migration_job_status","arguments":{"job_id":reindex_id}}),
        )
        .await;
        assert_eq!(status["result"]["isError"], false, "{status}");
        let status = tool_result_json(&status);
        assert_eq!(status["job_name"], "reindex");
        let rest = request
            .get(&format!("/api/migration-jobs/{reindex_id}"))
            .add_header("Authorization", format!("Bearer {migration_key}"))
            .await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(rest.json::<Value>()["job_name"], status["job_name"]);
        assert_eq!(rest.json::<Value>()["job_id"], status["job_id"]);

        let audit = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {audit_key}"))
            .await;
        assert_eq!(
            audit.status_code(),
            StatusCode::OK,
            "response: {}",
            audit.text()
        );
        let entries: Vec<Value> = audit.json();
        assert!(
            entries
                .iter()
                .any(|entry| entry["action"] == "reindex_embeddings")
        );
        let fill_entry = entries
            .iter()
            .find(|entry| entry["action"] == "fill_defaults")
            .expect("fill_defaults audit record");
        assert_eq!(fill_entry["detail"]["schema_name"], "note");
        assert_eq!(fill_entry["detail"]["job_id"], job_id.to_string());
        assert_eq!(fill_entry["detail"]["entities_updated"], 1);
        assert_eq!(fill_entry["detail"]["fields_filled"], 1);
    })
    .await;
}

/// Community MCP tools use the same authenticated mounted server as REST, and successful writes
/// commit before the tool result is returned. The follow-up reads make a dropped transaction fail.
#[tokio::test]
async fn community_mcp_entities_relations_and_schema_tools_execute_over_protocol() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { workspace_id, owner_id, api_key: schema_key, .. } = TestTenant::create(&request, &ctx, TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        ).await;
        let write_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Write, false).await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;

        let session = initialize(&request).await;
        let schema = mcp_call(
            &request,
            &session,
            &schema_key,
            2,
            "tools/call",
            json!({
                "name": "create_schema",
                "arguments": {
                    "definition": {
                        "name": "graph",
                        "entity_types": {
                            "person": { "fields": { "name": { "type": "string", "required": true } } }
                        },
                        "relation_types": {
                            "knows": { "source": "person", "target": "person" }
                        }
                    }
                }
            }),
        )
        .await;
        assert_eq!(schema["result"]["isError"], false, "MCP call: {schema}");

        let invalid = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({
                "name": "list_relations",
                "arguments": { "status": "not-a-status" }
            }),
        )
        .await;
        assert_eq!(invalid["result"]["isError"], true, "MCP call: {invalid}");
        assert!(invalid["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("relation status")));

        let created = mcp_call(
            &request,
            &session,
            &write_key,
            4,
            "tools/call",
            json!({
                "name": "create_entity",
                "arguments": {
                    "schema_name": "graph",
                    "entity_type": "person",
                    "data": { "name": "alice" }
                }
            }),
        )
        .await;
        assert_eq!(created["result"]["isError"], false, "MCP call: {created}");
        let alice = tool_result_json(&created);
        let alice_id: Uuid = alice["id"].as_str().unwrap().parse().unwrap();

        let second = mcp_call(
            &request,
            &session,
            &write_key,
            5,
            "tools/call",
            json!({
                "name": "create_entity",
                "arguments": {
                    "schema_name": "graph",
                    "entity_type": "person",
                    "data": { "name": "bob" }
                }
            }),
        )
        .await;
        let bob_id: Uuid = tool_result_json(&second)["id"].as_str().unwrap().parse().unwrap();

        let relation = mcp_call(
            &request,
            &session,
            &write_key,
            6,
            "tools/call",
            json!({
                "name": "create_relation",
                "arguments": {
                    "source_id": alice_id,
                    "target_id": bob_id,
                    "relation_type": "knows"
                }
            }),
        )
        .await;
        assert_eq!(relation["result"]["isError"], false, "MCP call: {relation}");
        let relation_id = tool_result_json(&relation)["id"].clone();

        let listed = mcp_call(
            &request,
            &session,
            &read_key,
            7,
            "tools/call",
            json!({ "name": "list_entities", "arguments": { "entity_type": "person" } }),
        )
        .await;
        assert_eq!(tool_result_json(&listed).as_array().unwrap().len(), 2);

        let status = mcp_call(
            &request,
            &session,
            &write_key,
            8,
            "tools/call",
            json!({
                "name": "set_relation_status",
                "arguments": { "id": relation_id, "status": "archived" }
            }),
        )
        .await;
        assert_eq!(tool_result_json(&status)["status"], "archived");

        let persisted = mcp_call(
            &request,
            &session,
            &read_key,
            9,
            "tools/call",
            json!({ "name": "get_relation", "arguments": { "id": relation_id } }),
        )
        .await;
        assert_eq!(tool_result_json(&persisted)["status"], "archived");

        let missing = mcp_call(
            &request,
            &session,
            &read_key,
            10,
            "tools/call",
            json!({ "name": "get_entity", "arguments": { "id": Uuid::new_v4() } }),
        )
        .await;
        assert_eq!(missing["result"]["isError"], true, "MCP call: {missing}");
        assert!(missing["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("not found")));
    })
    .await;
}

#[tokio::test]
async fn mcp_import_rolls_back_prior_records_when_a_later_line_is_invalid() {
    super::fill_defaults::boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            api_key: schema_key,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let session = initialize(&request).await;
        let schema_id = Uuid::new_v4();
        let jsonl = format!(
            "{}\nnot-json\n",
            json!({
                "kind": "schema",
                "record": {
                    "id": schema_id,
                    "tenant_id": tenant_id,
                    "workspace_id": workspace_id,
                    "name": "rollback-schema",
                    "version": 1,
                    "definition": {
                        "name": "rollback-schema",
                        "entity_types": { "note": { "fields": {} } },
                        "relation_types": {}
                    },
                    "status": "active",
                    "origin_template_id": null,
                    "origin_status": "detached",
                    "origin_snapshot": null,
                    "origin_updated_at": null,
                    "created_at": Utc::now()
                }
            })
        );
        let result = mcp_call(
            &request,
            &session,
            &schema_key,
            2,
            "tools/call",
            json!({ "name": "import_jsonl", "arguments": { "jsonl": jsonl } }),
        )
        .await;
        let accepted = tool_result_json(&result);
        let id = accepted["job_id"].as_str().unwrap().parse().unwrap();
        super::fill_defaults::fail_via_api(
            &request,
            &ctx,
            id,
            &schema_key,
            &format!("/api/import-jobs/{id}"),
        )
        .await;
        let status = mcp_call(
            &request,
            &session,
            &schema_key,
            3,
            "tools/call",
            json!({"name": "import_job_status", "arguments": {"job_id": id}}),
        )
        .await;
        assert_eq!(tool_result_json(&status)["status"], "failed");
        assert!(
            schema_schemas::get_active_schema(&ctx.db, workspace_id, "rollback-schema")
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn mcp_rejects_missing_auth_and_scope_insufficient_writes() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            workspace_id,
            owner_id,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Read,
                ..Default::default()
            },
        )
        .await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;
        let session = initialize(&request).await;

        let missing = request
            .post("/mcp")
            .add_header("Accept", "application/json, text/event-stream")
            .add_header("MCP-Session-Id", session.clone())
            .add_header("MCP-Protocol-Version", "2025-03-26")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "create_entity",
                    "arguments": {
                        "schema_name": "missing",
                        "entity_type": "missing",
                        "data": {}
                    }
                }
            }))
            .await;
        assert_eq!(missing.status_code(), StatusCode::OK, "{}", missing.text());
        assert!(rpc_body(&missing.text()).get("error").is_some());

        let denied = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({
                "name": "create_entity",
                "arguments": {
                    "schema_name": "missing",
                    "entity_type": "missing",
                    "data": {}
                }
            }),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true, "MCP call: {denied}");
        assert!(
            denied["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("scope"))
        );
    })
    .await;
}

/// The MCP search tool takes the same path as `GET /api/search`: the server queues the query and a worker holding the model embeds it.
#[tokio::test]
async fn mcp_search_asks_a_worker_to_embed_the_query() {
    use super::query_worker::boot_with_query_worker;
    use crate::http_stub::{keyword_provider, unavailable_provider};
    use yorishiro::models::entity_embeddings;

    let (provider, _server, inputs) = keyword_provider(768).await;
    boot_with_query_worker(provider.clone(), |request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;
        let definition = serde_json::from_value(json!({
            "name": "note",
            "entity_types": { "note": { "fields": {
                "title": { "type": "string", "required": true, "x-embed": true }
            } } }
        }))
        .expect("definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, definition, None, None)
            .await
            .expect("create schema");
        let mut candidates = Vec::new();
        for title in ["alpha plan", "beta plan"] {
            let record = entity_entities::create(
                &ctx.db,
                workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: json!({ "title": title }),
                },
                None,
            )
            .await
            .expect("create entity");
            entity_embeddings::sync_embedding_for_record(
                &ctx.db,
                workspace_id,
                &record,
                provider.as_ref(),
            )
            .await
            .expect("embed entity");
            candidates.push(record.id);
        }

        let session = initialize(&request).await;
        let found = mcp_call(
            &request,
            &session,
            &read_key,
            2,
            "tools/call",
            json!({ "name": "search_entities", "arguments": { "query_text": "beta", "limit": 2 } }),
        )
        .await;
        assert_eq!(found["result"]["isError"], false, "MCP call: {found}");
        let hits = tool_result_json(&found);
        assert_eq!(
            hits[0]["entity"]["data"]["title"], "beta plan",
            "nearest entity first: {hits}"
        );
        let calls = inputs.lock().unwrap().len();
        let (failed_provider, _failed_server) = unavailable_provider().await;
        let reindex =
            entity_embeddings::reindex_workspace(&ctx.db, workspace_id, failed_provider.as_ref())
                .await
                .expect("record failed reindex");
        assert_eq!(reindex.failures.len(), candidates.len());
        let unavailable = mcp_call(
            &request,
            &session,
            &read_key,
            4,
            "tools/call",
            json!({ "name": "search_entities", "arguments": { "query_text": "beta" } }),
        )
        .await;
        assert_eq!(unavailable["result"]["isError"], true, "{unavailable}");
        assert!(
            unavailable["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("reindexed")
        );
        let rest = request
            .get("/api/search?query_text=beta")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(rest.status_code(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(rest.json::<Value>()["error"]["code"], "backend_unavailable");
        assert_eq!(inputs.lock().unwrap().len(), calls);
        let recovered =
            entity_embeddings::reindex_workspace(&ctx.db, workspace_id, provider.as_ref())
                .await
                .expect("recover reindex");
        assert!(recovered.failures.is_empty());
        // The worker's provider is replaced by one that fails: the tool reports the worker's diagnostic.
        ctx.shared_store.insert(failed_provider);
        let failed = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({ "name": "search_entities", "arguments": { "query_text": "alpha" } }),
        )
        .await;
        assert_eq!(failed["result"]["isError"], true, "MCP call: {failed}");
        assert!(
            failed["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("HTTP 503"))
        );
        assert!(!failed.to_string().contains("private upstream diagnostic"));
    })
    .await;
}
