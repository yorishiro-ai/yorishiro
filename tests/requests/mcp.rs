use axum::http::StatusCode;
use axum_test::{TestResponse, TestServer};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::ee::services::licence::{LicenceClaims, LicenceState};
use yorishiro::models::_entities::template_templates;
use yorishiro::models::{entity_entities, schema_schemas, template_templates as templates};
use yorishiro::services::auth::ApiKeyScope;

use super::boot_request;
use super::fixtures::{self, TenantArgs, issue_api_key};

fn rpc_body(response: &TestResponse) -> Value {
    let text = response.text();
    let payload = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .find(|line| !line.is_empty())
        .unwrap_or(text.trim());
    serde_json::from_str(payload).unwrap_or_else(|err| panic!("invalid MCP response {err}: {text}"))
}

async fn initialize(request: &TestServer) -> String {
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
    let _ = rpc_body(&response);
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
    rpc_body(&response)
}

fn tool_inventory(response: &Value) -> Vec<Value> {
    response["result"]["tools"]
        .as_array()
        .expect("tools/list result")
        .clone()
}

fn install_licence(ctx: &loco_rs::app::AppContext, active: bool) {
    let state = if active {
        LicenceState::licensed(LicenceClaims {
            sub: "acme-corp".into(),
            plan: "enterprise".into(),
            exp: Utc::now().timestamp() + 60 * 60,
        })
    } else {
        LicenceState::default()
    };
    ctx.shared_store.insert(std::sync::Arc::new(state)
        as std::sync::Arc<dyn yorishiro::services::edition::EnterpriseEdition>);
}

fn tool_result_json(response: &Value) -> Value {
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool result text"),
    )
    .expect("JSON tool result")
}

/// A server instance is retained by rmcp for the whole session.
/// Licence changes must therefore gate discovery and dispatch dynamically, not only at construction.
#[tokio::test]
async fn mcp_origin_tools_disappear_after_same_session_licence_expiry() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        install_licence(&ctx, true);
        let (_tenant_id, workspace_id, owner_id, _schema_key) =
            fixtures::create_tenant_workspace_owner(
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

        install_licence(&ctx, false);
        let community = mcp_call(&request, &session, &key, 3, "tools/list", json!({})).await;
        let community_tools = tool_inventory(&community);
        for name in ["list_upstream_changes", "merge_preview", "merge_apply"] {
            assert!(!community_tools.iter().any(|tool| tool["name"] == name));
            let denied = mcp_call(
                &request,
                &session,
                &key,
                10,
                "tools/call",
                json!({ "name": name, "arguments": {} }),
            )
            .await;
            assert!(denied.get("error").is_some(), "tool executed: {denied}");
        }
    })
    .await;
}

/// Origin tools read templates through the control-plane connection, keep schema work on an
/// RLS-scoped transaction, enforce read/schema scopes, and explicitly commit a successful merge.
#[tokio::test]
async fn origin_mcp_tools_enforce_scope_and_commit_the_merge() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        install_licence(&ctx, true);
        let (tenant_id, workspace_id, owner_id, schema_key) =
            fixtures::create_tenant_workspace_owner(
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
        let template = template_templates::ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id),
            name: ActiveValue::Set("library-note".into()),
            definition: ActiveValue::Set(original),
            visibility: ActiveValue::Set("tenant".into()),
            tags: ActiveValue::Set(vec![]),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert library template");

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {schema_key}"))
            .json(&json!({ "template_id": template.id.to_string() }))
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

        let updated = serde_json::from_value(json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "category": { "type": "string" }
                } }
            }
        }))
        .expect("parse updated definition");
        let template_txn = ctx.db.begin().await.expect("begin template update");
        templates::update_template(
            &template_txn,
            tenant_id,
            template.id,
            templates::UpdateTemplateInput {
                name: None,
                description: None,
                definition: Some(updated),
                tags: None,
                locale: None,
            },
        )
        .await
        .expect("update template");
        template_txn.commit().await.expect("commit template update");

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
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_id, owner_id, migration_key) =
            fixtures::create_tenant_workspace_owner(
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
        assert_eq!(report["entities_updated"], 1);
        assert_eq!(report["fields_filled"], 1);

        let updated = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read filled entity");
        assert_eq!(updated.data, json!({ "title": "before", "priority": 7 }));
        assert_eq!(updated.schema_version, 1);
        let job_id: Uuid = report["job_id"].as_str().unwrap().parse().unwrap();

        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {migration_key}"))
            .await;
        assert_eq!(
            undo.status_code(),
            StatusCode::OK,
            "response: {}",
            undo.text()
        );
        let restored = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read undone entity");
        assert_eq!(restored.data, json!({ "title": "before" }));
        assert_eq!(restored.schema_version, 1);

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
