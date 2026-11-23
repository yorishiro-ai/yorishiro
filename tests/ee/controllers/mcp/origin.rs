use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::schema_schemas;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

/// A server instance is retained by rmcp for the whole session.
/// Origin operations remain ungated, like their REST counterparts, in an existing session.
#[tokio::test]
async fn mcp_origin_tools_match_ungated_rest_when_same_session_licence_state_is_missing() {
    boot_request::<App, _, _>(|request, ctx| async move {
        if crate::requests::skip_unlicensed(&ctx) {
            return;
        }
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
        for tool in licensed_tools {
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
#[expect(
    clippy::too_many_lines,
    reason = "the same schema and MCP session verify scope restrictions before previewing and committing the upstream merge"
)]
async fn origin_mcp_tools_enforce_scope_and_commit_the_merge() {
    boot_request::<App, _, _>(|request, ctx| async move {
        if crate::requests::skip_unlicensed(&ctx) {
            return;
        }
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id,
            api_key: schema_key,
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let administrator =
            crate::requests::fixtures::create_iam_administrator(&ctx, tenant_id).await;
        let read_key =
            issue_api_key(&ctx, workspace_id, administrator, ApiKeyScope::Read, false).await;

        let library_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant_id, owner_id).await;
        let original = json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        });
        let template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {library_key}"))
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
            .add_header("Authorization", format!("Bearer {library_key}"))
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
