use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::export::export_all;
use crate::models::import::{ImportResult, import_jsonl};
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

/// Renders an `ExportRecord` list to the JSONL text `import_jsonl` reads, exactly as `GET /api/export.jsonl` does.
fn to_jsonl(records: &[crate::models::export::ExportRecord]) -> String {
    let mut body = String::new();
    for record in records {
        body.push_str(&serde_json::to_string(record).unwrap());
        body.push('\n');
    }
    body
}

#[sqlx::test(migrations = "../../migrations")]
async fn imports_schema_entities_and_relations_from_jsonl(pool: PgPool) {
    let (source_tenant, source_workspace) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut source_conn = db
        .acquire_for_workspace(source_tenant, source_workspace)
        .await
        .unwrap();

    schemas::create_schema(
        &mut source_conn,
        source_tenant,
        source_workspace,
        task_schema(),
    )
    .await
    .unwrap();
    let a = entities::create(
        &mut source_conn,
        source_workspace,
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
        &mut source_conn,
        source_workspace,
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
        &mut *source_conn,
        source_workspace,
        CreateRelationInput {
            source_id: a.id,
            target_id: b.id,
            relation_type: "blocks".into(),
            properties: json!({ "note": "a blocks b" }),
        },
    )
    .await
    .unwrap();

    let records = export_all(&mut *source_conn, source_workspace)
        .await
        .unwrap();
    let jsonl = to_jsonl(&records);

    // Import into a *different*, empty workspace/tenant.
    let (dest_tenant, dest_workspace) = seed_workspace(&pool).await;
    let mut dest_conn = db
        .acquire_for_workspace(dest_tenant, dest_workspace)
        .await
        .unwrap();

    let result = import_jsonl(
        &mut dest_conn,
        dest_tenant,
        dest_workspace,
        jsonl.as_bytes(),
    )
    .await
    .unwrap();

    assert_eq!(result.schemas, 1);
    assert_eq!(result.entities, 2);
    assert_eq!(result.relations, 1);
    assert!(result.errors.is_empty());

    let dest_records = export_all(&mut *dest_conn, dest_workspace).await.unwrap();
    assert_eq!(dest_records.len(), 4);

    // Relation endpoints were remapped to the newly generated entity IDs, not the originals (which don't even exist in this workspace).
    let imported_relation = dest_records
        .iter()
        .find_map(|r| match r {
            crate::models::export::ExportRecord::Relation(rel) => Some(rel),
            _ => None,
        })
        .expect("relation was imported");
    assert_ne!(imported_relation.source_id, a.id);
    assert_ne!(imported_relation.target_id, b.id);

    let imported_entity_ids: Vec<Uuid> = dest_records
        .iter()
        .filter_map(|r| match r {
            crate::models::export::ExportRecord::Entity(e) => Some(e.id),
            _ => None,
        })
        .collect();
    assert!(imported_entity_ids.contains(&imported_relation.source_id));
    assert!(imported_entity_ids.contains(&imported_relation.target_id));
}

#[sqlx::test(migrations = "../../migrations")]
async fn import_is_all_or_nothing_on_a_bad_relation_reference(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    // A schema line followed by a relation line whose endpoints don't exist.
    // The schema insert should be rolled back along with the failing relation.
    let schema_json = serde_json::to_value(task_schema()).unwrap();
    let jsonl = format!(
        "{}\n{}\n",
        json!({
            "kind": "schema",
            "record": {
                "id": Uuid::from_u128(1),
                "tenant_id": tenant_id,
                "name": "task-management",
                "version": 1,
                "definition": schema_json,
                "status": "active",
                "created_at": chrono::Utc::now(),
            }
        }),
        json!({
            "kind": "relation",
            "record": {
                "id": Uuid::from_u128(2),
                "workspace_id": workspace_id,
                "source_id": Uuid::from_u128(3),
                "target_id": Uuid::from_u128(4),
                "relation_type": "blocks",
                "properties": {},
                "created_at": chrono::Utc::now(),
            }
        }),
    );

    let err = import_jsonl(&mut conn, tenant_id, workspace_id, jsonl.as_bytes())
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("line 2"), "message was: {message}");

    let records = export_all(&mut *conn, workspace_id).await.unwrap();
    assert!(
        records.is_empty(),
        "schema insert from line 1 should have been rolled back too, got: {records:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn import_rejects_malformed_lines(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let err = import_jsonl(
        &mut conn,
        tenant_id,
        workspace_id,
        b"not json at all".as_slice(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("line 1"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn import_skips_blank_lines(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let schema_json = serde_json::to_value(task_schema()).unwrap();
    let line = json!({
        "kind": "schema",
        "record": {
            "id": Uuid::from_u128(1),
            "tenant_id": tenant_id,
            "name": "task-management",
            "version": 1,
            "definition": schema_json,
            "status": "active",
            "created_at": chrono::Utc::now(),
        }
    });
    let jsonl = format!("\n{line}\n\n");

    let result = import_jsonl(&mut conn, tenant_id, workspace_id, jsonl.as_bytes())
        .await
        .unwrap();
    assert_eq!(result.schemas, 1);
}

/// An import that touched nothing must report zeros and no errors rather than, say, a default that looks like a partial success.
#[test]
fn a_default_result_reports_nothing_imported_and_no_errors() {
    let result = ImportResult::default();

    assert_eq!(result.schemas, 0);
    assert_eq!(result.entities, 0);
    assert_eq!(result.relations, 0);
    assert!(result.errors.is_empty());
}

/// The counts are what a caller reports back to a user, so the serialised field names are part of the API and pinned here.
#[test]
fn the_serialised_shape_names_each_counted_kind() {
    let result = ImportResult {
        schemas: 1,
        entities: 2,
        relations: 3,
        errors: vec!["line 4: bad".into()],
    };

    let json = serde_json::to_value(&result).unwrap();

    assert_eq!(json["schemas"], 1);
    assert_eq!(json["entities"], 2);
    assert_eq!(json["relations"], 3);
    assert_eq!(json["errors"][0], "line 4: bad");
}
