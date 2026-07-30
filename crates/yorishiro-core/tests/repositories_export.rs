use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use yorishiro_core::db::TenantDb;
use yorishiro_core::metaschema::MetaSchemaDefinition;
use yorishiro_core::repositories::export::{ExportRecord, export_all};
use yorishiro_core::repositories::relations::CreateRelationInput;
use yorishiro_core::repositories::{entities, relations, schemas};
use yorishiro_core::test_support;

fn task_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string", "required": true } } }
        },
        "relation_types": {
            "blocks": { "source": "task", "target": "task" }
        }
    }))
    .unwrap()
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    test_support::seed_tenant_and_workspace(pool).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn exports_schemas_entities_and_relations_for_the_tenant(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, workspace_id, task_schema())
        .await
        .unwrap();
    let a = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "a" }),
        },
        None,
    )
    .await
    .unwrap();
    let b = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "b" }),
        },
        None,
    )
    .await
    .unwrap();
    relations::create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: a.id,
            target_id: b.id,
            relation_type: "blocks".into(),
            properties: json!(null),
        },
    )
    .await
    .unwrap();

    let records = export_all(&mut conn, workspace_id).await.unwrap();

    let schema_count = records
        .iter()
        .filter(|r| matches!(r, ExportRecord::Schema(_)))
        .count();
    let entity_count = records
        .iter()
        .filter(|r| matches!(r, ExportRecord::Entity(_)))
        .count();
    let relation_count = records
        .iter()
        .filter(|r| matches!(r, ExportRecord::Relation(_)))
        .count();
    assert_eq!(schema_count, 1);
    assert_eq!(entity_count, 2);
    assert_eq!(relation_count, 1);

    let json = serde_json::to_value(&records[0]).unwrap();
    assert_eq!(json["kind"], "schema");
    assert!(json["record"]["definition"].is_object());
}

#[sqlx::test(migrations = "../../migrations")]
async fn export_is_empty_for_a_tenant_with_no_data(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let records = export_all(&mut conn, workspace_id).await.unwrap();
    assert!(records.is_empty());
}
