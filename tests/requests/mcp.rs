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

fn rpc_body(text: &str) -> Value {
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
    let guard = EnvGuard::capture(&["YORISHIRO_MCP_ALLOWED_HOSTS"]);
    guard.set("YORISHIRO_MCP_ALLOWED_HOSTS", "mcp.remote.test");
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
/// Losing the boot-installed licence state must fail closed even in an existing session.
#[tokio::test]
async fn mcp_origin_tools_disappear_when_same_session_licence_state_is_missing() {
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

        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {migration_key}"))
            .await;
        assert_eq!(
            undo.status_code(),
            StatusCode::ACCEPTED,
            "response: {}",
            undo.text()
        );
        let undo_id = undo.json::<Value>()["job_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
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
        let reindex = entity_embeddings::reindex_workspace(
            &ctx.db,
            workspace_id,
            &candidates,
            failed_provider.as_ref(),
        )
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
        let recovered = entity_embeddings::reindex_workspace(
            &ctx.db,
            workspace_id,
            &candidates,
            provider.as_ref(),
        )
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
