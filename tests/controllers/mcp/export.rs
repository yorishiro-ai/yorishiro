use axum::http::StatusCode;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::_entities::entity_entities::{Column, Entity};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

use super::*;

#[tokio::test]
async fn export_jsonl_tool_matches_the_rest_export() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"export-history","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let created_schema = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        created_schema.assert_status(StatusCode::CREATED);
        let created = request.post("/api/entities").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&json!({"schema_name":"export-history","entity_type":"note","data":{"title":"exported"}})).await;
        created.assert_status(StatusCode::CREATED);
        let entity_id = created.json::<Value>()["entity_id"].as_str().unwrap().to_owned();

        let exported = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"export_workspace","arguments":{}})).await;
        let text = exported["result"]["content"][0]["text"].as_str().unwrap().to_owned();
        let mut kinds = std::collections::BTreeMap::new();
        let mut entity_rows = Vec::new();
        for line in text.lines() {
            let row: Value = serde_json::from_str(line).expect("export line is JSON");
            *kinds.entry(row["kind"].as_str().unwrap().to_owned()).or_insert(0) += 1;
            if row["record"]["entity_id"] == entity_id {
                entity_rows.push(row);
            }
        }
        assert!(kinds.get("schema").copied().unwrap_or(0) >= 1, "kinds: {kinds:?}");
        assert_eq!(kinds.get("entity").copied().unwrap_or(0), 1, "kinds: {kinds:?}");
        assert_eq!(entity_rows.len(), 1, "kinds: {kinds:?}");
        assert_eq!(entity_rows[0]["record"]["data"]["title"], "exported");

        // The REST endpoint and the tool return the same lines, in the same order.
        let rest = request.get("/api/export").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(text, rest.text());
        // Direct write (a): the entity API rejects a payload this large, so the 8 MiB JSON is stored directly.
        Entity::update_many()
            .col_expr(Column::Data, sea_orm::sea_query::Expr::value(json!({"title":"x".repeat(8 * 1024 * 1024)})))
            .filter(Column::Id.eq(entity_id.parse::<Uuid>().unwrap())).exec(&ctx.db).await.unwrap();
        let overflow = mcp_call(&request, &session, &tenant.api_key, 3, "tools/call", json!({"name":"export_workspace","arguments":{}})).await;
        assert_eq!(overflow["result"]["isError"], true);
        assert!(overflow.to_string().contains("8 MiB"));
    }).await;
}
