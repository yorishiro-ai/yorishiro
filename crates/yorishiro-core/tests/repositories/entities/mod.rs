use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::repositories::entities::{
    self, CreateEntityInput, DEFAULT_LIST_LIMIT, ListEntitiesQuery,
};
use crate::repositories::schemas;
use crate::test_support;

fn task_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true }
                }
            }
        }
    }))
    .unwrap()
}

#[test]
fn default_list_query_uses_a_sensible_page_size() {
    let query = ListEntitiesQuery::default();
    assert_eq!(query.limit, DEFAULT_LIST_LIMIT);
    assert_eq!(query.offset, 0);
    assert!(query.entity_type.is_none());
}

#[test]
fn missing_required_field_points_at_the_missing_property() {
    let def = task_schema();
    let entity_type_def = &def.entity_types["task"];

    let err = entities::validate_data(entity_type_def, &json!({})).unwrap_err();
    match err {
        YorishiroError::ValidationFailed { details, .. } => {
            assert!(
                details.iter().any(|d| d.field == "/title"),
                "details: {details:?}"
            );
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    test_support::seed_tenant_and_workspace(pool).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_and_fetches_entity(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
        .await
        .unwrap();

    let created = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "buy milk" }),
        },
        None,
    )
    .await
    .unwrap();

    assert_eq!(created.entity_type, "task");
    assert_eq!(created.schema_version, 1);

    let fetched = entities::get(&mut conn, workspace_id, created.id)
        .await
        .unwrap();
    assert_eq!(fetched.data["title"], "buy milk");
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_invalid_data(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
        .await
        .unwrap();

    let err = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({}),
        },
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::ValidationFailed { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_unknown_entity_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
        .await
        .unwrap();

    let err = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "nonexistent".into(),
            data: json!({}),
        },
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn enforces_tenant_isolation(pool: PgPool) {
    let (tenant_a_tenant, tenant_a) = seed_workspace(&pool).await;
    let (tenant_b_tenant, tenant_b) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);

    let mut conn_a = db
        .acquire_for_workspace(tenant_a_tenant, tenant_a)
        .await
        .unwrap();
    schemas::create_schema(&mut conn_a, tenant_a_tenant, tenant_a, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn_a,
        tenant_a,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "tenant a task" }),
        },
        None,
    )
    .await
    .unwrap();

    let mut conn_b = db
        .acquire_for_workspace(tenant_b_tenant, tenant_b)
        .await
        .unwrap();
    let result = entities::get(&mut conn_b, tenant_b, entity.id).await;
    assert!(matches!(result, Err(YorishiroError::NotFound { .. })));
}

#[sqlx::test(migrations = "../../migrations")]
async fn update_validates_against_creation_time_schema_version(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "v1 task" }),
        },
        None,
    )
    .await
    .unwrap();

    let v2: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "priority": { "type": "integer", "required": true }
                }
            }
        }
    }))
    .unwrap();
    schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, v2)
        .await
        .unwrap();

    let updated = entities::update(
        &mut conn,
        workspace_id,
        entity.id,
        json!({ "title": "v1 task updated" }),
        None,
    )
    .await
    .unwrap();
    assert_eq!(updated.schema_version, 1);
    assert_eq!(updated.data["title"], "v1 task updated");
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_removes_entity(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "to delete" }),
        },
        None,
    )
    .await
    .unwrap();

    entities::delete(&mut conn, workspace_id, entity.id)
        .await
        .unwrap();
    let err = entities::get(&mut conn, workspace_id, entity.id)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_by_entity_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(
        &mut conn,
        workspace_id_tenant,
        workspace_id,
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } },
                "project": { "fields": { "title": { "type": "string", "required": true } } }
            }
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    for (entity_type, title) in [
        ("task", "task one"),
        ("task", "task two"),
        ("project", "project one"),
    ] {
        entities::create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: entity_type.into(),
                data: json!({ "title": title }),
            },
            None,
        )
        .await
        .unwrap();
    }

    let tasks = entities::list(
        &mut conn,
        workspace_id,
        ListEntitiesQuery {
            entity_type: Some("task".into()),
            filter: None,
            schema_version: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|e| e.entity_type == "task"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_by_data_field_value(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
        .await
        .unwrap();

    for (title, status) in [
        ("task one", "active"),
        ("task two", "done"),
        ("task three", "active"),
    ] {
        entities::create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": title, "status": status }),
            },
            None,
        )
        .await
        .unwrap();
    }

    let active = entities::list(
        &mut conn,
        workspace_id,
        ListEntitiesQuery {
            entity_type: None,
            filter: Some(json!({ "status": "active" })),
            schema_version: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!(active.len(), 2);
    assert!(active.iter().all(|e| e.data["status"] == "active"));
}

/// Entities record the schema version they were written against and keep it when a newer version is created, so filtering by version selects what a given version actually produced.
/// The distinction only shows up once two versions exist and each has entities of its own.
#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_by_schema_version(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let (v1, _) =
        schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
            .await
            .unwrap();
    entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "written against v1" }),
        },
        None,
    )
    .await
    .unwrap();

    // A second version of the same schema.
    // The entity above keeps schema_version = 1.
    let (v2, _) =
        schemas::create_schema(&mut conn, workspace_id_tenant, workspace_id, task_schema())
            .await
            .unwrap();
    assert_eq!(v2.version, v1.version + 1);

    for title in ["first against v2", "second against v2"] {
        entities::create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": title }),
            },
            None,
        )
        .await
        .unwrap();
    }

    let from_v1 = entities::list(
        &mut conn,
        workspace_id,
        ListEntitiesQuery {
            schema_version: Some(v1.version),
            limit: 10,
            ..ListEntitiesQuery::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(from_v1.len(), 1);
    assert_eq!(from_v1[0].data["title"], "written against v1");

    let from_v2 = entities::list(
        &mut conn,
        workspace_id,
        ListEntitiesQuery {
            schema_version: Some(v2.version),
            limit: 10,
            ..ListEntitiesQuery::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(from_v2.len(), 2);

    // Without the filter, every version's entities come back.
    let all = entities::list(
        &mut conn,
        workspace_id,
        ListEntitiesQuery {
            limit: 10,
            ..ListEntitiesQuery::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(all.len(), 3);
}

/// A workspace with no schema refuses entity writes, and says that is why.
/// Before the status column this failed too, but as a 404 on the schema name: which reads as a typo.
#[sqlx::test(migrations = "../../migrations")]
async fn refuses_entity_creation_while_the_workspace_has_no_schema(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let err = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "too early" }),
        },
        None,
    )
    .await
    .unwrap_err();

    match err {
        YorishiroError::ValidationFailed { hint, .. } => {
            assert!(
                hint.contains("create a schema first"),
                "the hint should say what to do, got {hint:?}"
            );
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// The first schema lifts the block, and entities can be written from then on.
#[sqlx::test(migrations = "../../migrations")]
async fn creating_the_first_schema_activates_the_workspace(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    assert!(
        crate::repositories::tenancy::is_schema_pending(&mut conn, workspace_id)
            .await
            .unwrap(),
        "a fresh workspace starts pending"
    );

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();

    assert!(
        !crate::repositories::tenancy::is_schema_pending(&mut conn, workspace_id)
            .await
            .unwrap(),
        "the first schema activates it"
    );

    // And the write that was refused above now succeeds.
    entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "now fine" }),
        },
        None,
    )
    .await
    .unwrap();
}

/// A second schema version must not flip the workspace back or otherwise disturb it: the activation runs on every create_schema call, so it has to be idempotent.
#[sqlx::test(migrations = "../../migrations")]
async fn a_further_schema_version_leaves_the_workspace_active(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();

    assert!(
        !crate::repositories::tenancy::is_schema_pending(&mut conn, workspace_id)
            .await
            .unwrap()
    );
}

fn task_schema_with_category() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "category": { "type": "string", "required": true }
                }
            }
        }
    }))
    .unwrap()
}

/// The case the endpoint exists for: an entity written before a field existed.
/// Without this a reader cannot tell the field was never available from the field being left blank.
#[sqlx::test(migrations = "../../migrations")]
async fn drift_reports_fields_the_entity_predates(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "written before category existed" }),
        },
        None,
    )
    .await
    .unwrap();

    // Adding a required field is a breaking change, so this lands as a new version and the entity above stays on the old one.
    schemas::create_schema(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema_with_category(),
    )
    .await
    .unwrap();

    let drift = entities::drift(&mut conn, workspace_id, entity.id)
        .await
        .unwrap();

    assert_eq!(drift.schema_version, 1);
    assert_eq!(drift.active_schema_version, 2);
    let names: Vec<&str> = drift
        .missing_fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, vec!["category"], "only the new field is missing");
    assert!(
        drift.missing_fields[0].required,
        "required is what makes this worth reporting"
    );
}

/// An entity on the active version has nothing to report, and says so with an empty list rather than by omitting the field.
#[sqlx::test(migrations = "../../migrations")]
async fn drift_is_empty_for_a_current_entity(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "current" }),
        },
        None,
    )
    .await
    .unwrap();

    let drift = entities::drift(&mut conn, workspace_id, entity.id)
        .await
        .unwrap();

    assert_eq!(drift.schema_version, drift.active_schema_version);
    assert!(drift.missing_fields.is_empty());
}

/// Every create_schema call makes a new version, breaking or not, so an optional addition leaves earlier entities behind too.
/// They are reported as missing but not required, which is the distinction a caller acts on: nothing is invalid, there is just newer structure available.
#[sqlx::test(migrations = "../../migrations")]
async fn drift_marks_an_optional_addition_as_not_required(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "before the optional field" }),
        },
        None,
    )
    .await
    .unwrap();

    let optional_added: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "tag":   { "type": "string" }
                }
            }
        }
    }))
    .unwrap();
    schemas::create_schema(&mut conn, tenant_id, workspace_id, optional_added)
        .await
        .unwrap();

    let drift = entities::drift(&mut conn, workspace_id, entity.id)
        .await
        .unwrap();

    assert_eq!(drift.missing_fields.len(), 1);
    assert_eq!(drift.missing_fields[0].name, "tag");
    assert!(
        !drift.missing_fields[0].required,
        "an optional addition leaves the entity valid; required is what separates the two"
    );
}

/// The number an operator acts on: entities lacking a field the active version requires.
/// Entities merely behind, but still valid, are counted separately: conflating them would inflate the work a migration appears to need.
#[sqlx::test(migrations = "../../migrations")]
async fn dry_run_separates_entities_that_need_values_from_ones_merely_behind(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    for title in ["one", "two"] {
        entities::create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": title }),
            },
            None,
        )
        .await
        .unwrap();
    }

    // An optional addition: the entities stay valid, only their version marker falls behind.
    let optional_added: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "tag":   { "type": "string" }
                }
            }
        }
    }))
    .unwrap();
    schemas::create_schema(&mut conn, tenant_id, workspace_id, optional_added)
        .await
        .unwrap();

    let report = entities::migration_dry_run(&mut conn, workspace_id, "task-management")
        .await
        .unwrap();

    assert_eq!(report.total_entities, 2);
    assert_eq!(report.behind_but_valid, 2, "an optional field is not work");
    assert_eq!(report.needs_values, 0);

    // Now a required addition: the same entities become work.
    schemas::create_schema(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema_with_category(),
    )
    .await
    .unwrap();

    let report = entities::migration_dry_run(&mut conn, workspace_id, "task-management")
        .await
        .unwrap();

    assert_eq!(report.needs_values, 2);
    assert_eq!(report.behind_but_valid, 0);
    let by_type = &report.by_entity_type[0];
    assert_eq!(by_type.entity_type, "task");
    assert_eq!(
        by_type.missing_required,
        vec!["category"],
        "the report names the work, not just its size"
    );
}

/// Entities already on the active version are counted as current, and a workspace with nothing behind reports no work.
#[sqlx::test(migrations = "../../migrations")]
async fn dry_run_reports_nothing_to_do_when_everything_is_current(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "current" }),
        },
        None,
    )
    .await
    .unwrap();

    let report = entities::migration_dry_run(&mut conn, workspace_id, "task-management")
        .await
        .unwrap();

    assert_eq!(report.total_entities, 1);
    assert_eq!(report.current, 1);
    assert_eq!(report.needs_values, 0);
    assert!(report.by_entity_type.is_empty());
}

/// The point of a snapshot: what the row actually held, restorable afterwards.
/// Entity updates are last-write-wins, so without this the previous data is simply gone.
#[sqlx::test(migrations = "../../migrations")]
async fn a_snapshot_restores_what_the_entity_held(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "before" }),
        },
        None,
    )
    .await
    .unwrap();

    let job_id = uuid::Uuid::nil();
    entities::snapshot(&mut conn, workspace_id, entity.id, job_id)
        .await
        .unwrap();

    entities::update(
        &mut conn,
        workspace_id,
        entity.id,
        json!({ "title": "after" }),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        entities::get(&mut conn, workspace_id, entity.id)
            .await
            .unwrap()
            .data["title"],
        "after"
    );

    let report = entities::undo_job(&mut conn, workspace_id, job_id)
        .await
        .unwrap();
    assert_eq!(report.restored, 1);
    assert_eq!(report.missing, 0);

    assert_eq!(
        entities::get(&mut conn, workspace_id, entity.id)
            .await
            .unwrap()
            .data["title"],
        "before",
        "the entity holds what it held before the overwrite"
    );
}

/// Undoing the same job twice would restore stale data over whatever came after, so the snapshots go with the undo and the second attempt finds nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_job_cannot_be_undone_twice(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "original" }),
        },
        None,
    )
    .await
    .unwrap();

    let job_id = uuid::Uuid::nil();
    entities::snapshot(&mut conn, workspace_id, entity.id, job_id)
        .await
        .unwrap();
    entities::undo_job(&mut conn, workspace_id, job_id)
        .await
        .unwrap();

    let err = entities::undo_job(&mut conn, workspace_id, job_id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, YorishiroError::NotFound { .. }),
        "got {err:?}"
    );
}

/// An entity deleted after its snapshot is counted, not fatal.
/// Refusing the whole undo because one row is gone would leave every other entity in the job wrong.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_entity_is_counted_rather_than_failing_the_undo(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let job_id = uuid::Uuid::nil();
    let mut ids = Vec::new();
    for title in ["kept", "deleted"] {
        let e = entities::create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": title }),
            },
            None,
        )
        .await
        .unwrap();
        entities::snapshot(&mut conn, workspace_id, e.id, job_id)
            .await
            .unwrap();
        ids.push(e.id);
    }

    entities::delete(&mut conn, workspace_id, ids[1])
        .await
        .unwrap();

    let report = entities::undo_job(&mut conn, workspace_id, job_id)
        .await
        .unwrap();
    assert_eq!(report.restored, 1);
    assert_eq!(report.missing, 1, "the deleted one is reported, not fatal");
}

fn task_schema_with_defaulted_field() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title":  { "type": "string", "required": true },
                    "status": { "type": "string", "required": true, "default": "todo" }
                }
            }
        }
    }))
    .unwrap()
}

/// Mode A: a field added later, with a default, is filled into the entities that predate it, and those entities keep their own version, because filling a value is not migrating between definitions.
#[sqlx::test(migrations = "../../migrations")]
async fn fill_defaults_fills_predating_entities_without_moving_their_version(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "written first" }),
        },
        None,
    )
    .await
    .unwrap();
    let original_version = entity.schema_version;

    schemas::create_schema(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema_with_defaulted_field(),
    )
    .await
    .unwrap();

    let job_id = uuid::Uuid::nil();
    let report = entities::fill_defaults(&mut conn, workspace_id, "task-management", job_id)
        .await
        .unwrap();

    assert_eq!(report.filled, 1);
    assert_eq!(report.skipped_no_default, 0);

    let after = entities::get(&mut conn, workspace_id, entity.id)
        .await
        .unwrap();
    assert_eq!(after.data["status"], "todo");
    assert_eq!(
        after.data["title"], "written first",
        "existing data is kept"
    );
    assert_eq!(
        after.schema_version, original_version,
        "filling a value does not move the entity between versions"
    );
}

/// A required field with no default is left alone and reported.
/// Inventing a value would make it indistinguishable from one somebody chose.
#[sqlx::test(migrations = "../../migrations")]
async fn fill_defaults_leaves_fields_with_no_default_alone(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "written first" }),
        },
        None,
    )
    .await
    .unwrap();

    // `category` is required and has no default.
    schemas::create_schema(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema_with_category(),
    )
    .await
    .unwrap();

    let report = entities::fill_defaults(
        &mut conn,
        workspace_id,
        "task-management",
        uuid::Uuid::nil(),
    )
    .await
    .unwrap();

    assert_eq!(report.filled, 0);
    assert_eq!(report.skipped_no_default, 1);
    assert_eq!(report.still_missing, vec!["category"]);

    let after = entities::get(&mut conn, workspace_id, entity.id)
        .await
        .unwrap();
    assert!(
        after.data.get("category").is_none(),
        "no invented value: {:?}",
        after.data
    );
}

/// The run is undoable as one, which is what the job id is for.
#[sqlx::test(migrations = "../../migrations")]
async fn a_fill_can_be_undone_as_one_job(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    for title in ["one", "two"] {
        entities::create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": title }),
            },
            None,
        )
        .await
        .unwrap();
    }
    schemas::create_schema(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema_with_defaulted_field(),
    )
    .await
    .unwrap();

    let job_id = uuid::Uuid::nil();
    let report = entities::fill_defaults(&mut conn, workspace_id, "task-management", job_id)
        .await
        .unwrap();
    assert_eq!(report.filled, 2);

    let undo = entities::undo_job(&mut conn, workspace_id, job_id)
        .await
        .unwrap();
    assert_eq!(undo.restored, 2);

    let listed = entities::list(&mut conn, workspace_id, ListEntitiesQuery::default())
        .await
        .unwrap();
    for entity in listed {
        assert!(
            entity.data.get("status").is_none(),
            "the fill was put back: {:?}",
            entity.data
        );
    }
}

/// Snapshots age out, so a workspace that migrates repeatedly does not accumulate before-images without bound.
/// `YORISHIRO_SNAPSHOT_RETENTION_DAYS` defaults to 30; this backdates one past that and runs a second job, which is when the sweep happens.
#[sqlx::test(migrations = "../../migrations")]
async fn a_migration_drops_the_snapshots_that_aged_out(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, tenant_id, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "kept" }),
        },
        None,
    )
    .await
    .unwrap();

    let old_job = uuid::Uuid::nil();
    entities::snapshot(&mut conn, workspace_id, entity.id, old_job)
        .await
        .unwrap();

    // Backdated rather than waited for: the sweep compares against `now()`, and a test cannot spend 31 days proving it.
    sqlx::query(
        "UPDATE content.entity_snapshots SET created_at = now() - INTERVAL '31 days' \
         WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .execute(&mut *conn)
    .await
    .unwrap();

    // Any migration job runs the sweep.
    // This one finds nothing to fill, which is enough.
    entities::fill_defaults(
        &mut conn,
        workspace_id,
        "task-management",
        uuid::Uuid::max(),
    )
    .await
    .unwrap();

    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM content.entity_snapshots WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(remaining, 0, "the sweep took the aged-out image");

    // An expired window answers the same way a job that never existed does: which is what "undoable for N days" means once the days are up.
    let err = entities::undo_job(&mut conn, workspace_id, old_job)
        .await
        .unwrap_err();
    assert!(
        matches!(err, YorishiroError::NotFound { .. }),
        "undoing past the window reports the job as gone, got {err:?}"
    );
}

/// A retention value that does not fit `make_interval(days => …)` must not reach it.
/// Parsed as `i64` and cast, `2147483648` wraps negative: `now() - a negative interval` puts the cutoff in the *future*, and the sweep would delete the images it exists to preserve.
#[test]
fn an_out_of_range_retention_falls_back_to_the_default() {
    // Serialized against the other env-reading tests in this crate is not needed: this reads a key nothing else touches, and reads it through the same function the sweep uses.
    let restore = std::env::var_os("YORISHIRO_SNAPSHOT_RETENTION_DAYS");

    for value in ["2147483648", "9999999999999999999", "not-a-number", ""] {
        // SAFETY: single-threaded test, and no other test reads this key.
        unsafe { std::env::set_var("YORISHIRO_SNAPSHOT_RETENTION_DAYS", value) };
        assert_eq!(
            entities::snapshot_retention_days(),
            30,
            "'{value}' does not parse as i32 and must fall back, never wrap"
        );
    }

    // A negative value does parse.
    // It is not clamped or rejected: `prune_snapshots` treats anything `<= 0` as "keep everything", so it lands with `0` rather than reaching `make_interval` and moving the cutoff into the future.
    // SAFETY: as above.
    unsafe { std::env::set_var("YORISHIRO_SNAPSHOT_RETENTION_DAYS", "-1") };
    assert!(entities::snapshot_retention_days() <= 0, "sweeping is off");

    // SAFETY: as above.
    unsafe {
        match restore {
            Some(v) => std::env::set_var("YORISHIRO_SNAPSHOT_RETENTION_DAYS", v),
            None => std::env::remove_var("YORISHIRO_SNAPSHOT_RETENTION_DAYS"),
        }
    }
}
