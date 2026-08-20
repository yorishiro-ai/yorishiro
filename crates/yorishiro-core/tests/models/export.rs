use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::export::{ExportRecord, export_all};
use crate::models::relations::CreateRelationInput;
use crate::models::{entities, relations, schemas};
use crate::test_support;

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
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
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
        &mut *conn,
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
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let records = export_all(&mut conn, workspace_id).await.unwrap();
    assert!(records.is_empty());
}

/// The export format is a tagged union so a reader can tell record kinds apart without tracking line position.
/// The tag and content keys are the on-disk format: pinned here because a rename would make every previously exported file unreadable.
#[test]
fn each_kind_is_tagged_so_a_reader_can_discriminate_lines() {
    let entity = ExportRecord::Entity(crate::models::entities::EntityRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        schema_id: uuid::Uuid::nil(),
        schema_version: 1,
        entity_type: "task".into(),
        data: serde_json::json!({ "title": "t" }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        created_by: None,
        updated_by: None,
    });

    let json = serde_json::to_value(&entity).unwrap();

    assert_eq!(json["kind"], "entity");
    assert_eq!(json["record"]["entity_type"], "task");
}

/// Import reads back exactly what export wrote, so the round trip through the tagged form has to preserve which variant a line was.
#[test]
fn a_record_round_trips_back_into_the_same_variant() {
    let relation = ExportRecord::Relation(crate::models::relations::RelationRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        source_id: uuid::Uuid::nil(),
        target_id: uuid::Uuid::nil(),
        relation_type: "blocks".into(),
        properties: serde_json::json!({}),
        status: "active".to_string(),
        created_at: chrono::Utc::now(),
    });

    let encoded = serde_json::to_string(&relation).unwrap();
    let decoded: ExportRecord = serde_json::from_str(&encoded).unwrap();

    assert!(matches!(decoded, ExportRecord::Relation(_)));
}
