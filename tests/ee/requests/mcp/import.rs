use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::schema_schemas;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn mcp_import_rolls_back_prior_records_when_a_later_line_is_invalid() {
    boot_request::<App, _, _>(|request, ctx| async move {
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
        crate::requests::fill_defaults::fail_via_api(
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
