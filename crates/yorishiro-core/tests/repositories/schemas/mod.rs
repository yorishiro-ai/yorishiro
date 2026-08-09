use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::repositories::schemas::{create_schema, get_active_schema, get_by_id};
use crate::test_support;

fn task_schema(with_priority: bool) -> MetaSchemaDefinition {
    let fields = if with_priority {
        json!({
            "title": { "type": "string", "required": true },
            "priority": { "type": "integer" }
        })
    } else {
        json!({ "title": { "type": "string", "required": true } })
    };
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": { "task": { "fields": fields } }
    }))
    .unwrap()
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    test_support::seed_tenant_and_workspace(pool).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_first_version_as_active(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let (record, diff) = create_schema(&mut conn, workspace_id_tenant, task_schema(false))
        .await
        .unwrap();
    assert_eq!(record.version, 1);
    assert_eq!(record.status, "active");
    assert!(!diff.is_breaking);
}

#[sqlx::test(migrations = "../../migrations")]
async fn creating_new_version_archives_previous_and_reports_breaking_diff(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let (v1, _) = create_schema(&mut conn, workspace_id_tenant, task_schema(false))
        .await
        .unwrap();

    let mut required_priority = task_schema(true);
    required_priority
        .entity_types
        .get_mut("task")
        .unwrap()
        .fields
        .get_mut("priority")
        .unwrap()
        .required = true;

    let (v2, diff) = create_schema(&mut conn, workspace_id_tenant, required_priority)
        .await
        .unwrap();
    assert_eq!(v2.version, 2);
    assert!(diff.is_breaking, "reasons: {:?}", diff.reasons);

    let archived = get_by_id(&mut conn, workspace_id_tenant, v1.id)
        .await
        .unwrap();
    assert_eq!(archived.status, "archived");

    let active = get_active_schema(&mut conn, workspace_id_tenant, "task-management")
        .await
        .unwrap();
    assert_eq!(active.id, v2.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_active_schema_reports_not_found_when_absent(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let err = get_active_schema(&mut conn, workspace_id_tenant, "does-not-exist")
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}
