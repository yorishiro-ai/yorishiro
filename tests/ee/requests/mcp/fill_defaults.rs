use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::{entity_entities, schema_schemas};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

/// The fill_defaults MCP path must be discoverable, execute with Migration scope, persist its
/// writes and snapshot, and leave an audit record. A lower-scope key receives an isError result
/// and cannot change the entity.
#[tokio::test]
async fn fill_defaults_mcp_executes_and_enforces_migration_scope() {
    use sea_orm::EntityTrait;
    boot_request::<App, _, _>(|request, ctx| async move {
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
        crate::requests::fill_defaults::finish_via_api(
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
        crate::requests::fill_defaults::finish_via_api(
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
