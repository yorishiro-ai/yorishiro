use axum::http::StatusCode;
use sea_orm::{ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::{
    App,
    models::{_entities::entity_entities::Column, entity_entities},
};

use super::{
    boot_request,
    fixtures::{TenantArgs, TestTenant},
};

#[tokio::test]
async fn export_pages_all_rows_and_rejects_byte_and_record_overflow() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let response = request.post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"name":"bounded-export","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}})).await;
        response.assert_status(StatusCode::CREATED);
        let schema_id: Uuid = serde_json::from_value(response.json::<Value>()["schema"]["id"].clone()).unwrap();
        let make_row = |id| entity_entities::ActiveModel {
            id: Set(id), workspace_id: Set(tenant.workspace_id), schema_id: Set(schema_id),
            schema_version: Set(1), entity_type: Set("note".into()), data: Set(json!({"title":"small"})),
            ..Default::default()
        };
        let mut ids: Vec<_> = (0..65).map(|_| Uuid::now_v7()).collect();
        ids.sort();
        entity_entities::Entity::insert_many(ids.iter().copied().map(make_row)).exec(&ctx.db).await.unwrap();
        let export = request.get("/api/export.jsonl")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        export.assert_status(StatusCode::OK);
        let rows: Vec<Value> = export.text().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(rows.len(), 66);
        assert_eq!(rows[0]["kind"], "schema");
        let exported: Vec<Uuid> = rows.iter().skip(1).map(|row| serde_json::from_value(row["record"]["id"].clone()).unwrap()).collect();
        assert_eq!(exported, ids);
        assert!(rows.iter().all(|row| row["record"]["embedding_sync_token"].is_null()));

        entity_entities::Entity::update_many()
            .col_expr(Column::Data, sea_orm::sea_query::Expr::value(json!({"title":"x".repeat(8 * 1024 * 1024)})))
            .filter(Column::Id.eq(ids[0])).exec(&ctx.db).await.unwrap();
        request.get("/api/export.jsonl")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        entity_entities::Entity::update_many()
            .col_expr(Column::Data, sea_orm::sea_query::Expr::value(json!({"title":"small"})))
            .filter(Column::Id.eq(ids[0])).exec(&ctx.db).await.unwrap();
        // Include the schema in the total record limit.
        for chunk in (65..10_000).collect::<Vec<_>>().chunks(256) {
            entity_entities::Entity::insert_many(chunk.iter().map(|_| make_row(Uuid::now_v7())))
                .exec(&ctx.db).await.unwrap();
        }
        let overflow = request.get("/api/export.jsonl")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        overflow.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        assert!(overflow.text().contains("10,000 records"));
    }).await;
}
