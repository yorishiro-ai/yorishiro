use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::entities;
use crate::models::relations::{
    CreateRelationInput, DEFAULT_LIST_LIMIT, DEFAULT_NEIGHBORS_LIMIT, ListRelationsQuery,
    RelationRecord, create, delete, get, list, neighbors, neighbors_batch, set_status,
};
use crate::models::schemas;
use crate::test_support;

fn project_task_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string", "required": true } } },
            "project": { "fields": { "title": { "type": "string", "required": true } } }
        },
        "relation_types": {
            "belongs_to": { "source": "task", "target": "project" }
        }
    }))
    .unwrap()
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    test_support::seed_tenant_and_workspace(pool).await
}

async fn seed_task_and_project(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> (entities::EntityRecord, entities::EntityRecord) {
    schemas::create_schema(conn, tenant_id, workspace_id, project_task_schema())
        .await
        .unwrap();

    let task = entities::create(
        conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report" }),
        },
        None,
    )
    .await
    .unwrap();

    let project = entities::create(
        conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "project".into(),
            data: json!({ "title": "Q3 roadmap" }),
        },
        None,
    )
    .await
    .unwrap();

    (task, project)
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_and_fetches_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    let created = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    assert_eq!(created.relation_type, "belongs_to");
    assert_eq!(created.properties, json!({}));

    let fetched = get(&mut conn, workspace_id, created.id).await.unwrap();
    assert_eq!(fetched.source_id, task.id);
    assert_eq!(fetched.target_id, project.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_relation_type_with_mismatched_source_target(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    // reversed: belongs_to expects source=task target=project, not the other way around.
    let err = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: project.id,
            target_id: task.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::RelationTypeMismatch { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_relation_with_nonexistent_source(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (_, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    let err = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: Uuid::nil(),
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_unknown_relation_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    let err = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "no_such_relation".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_duplicate_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let err = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::Conflict { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_entity_cascades_relation_deletion(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    let relation = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    entities::delete(&mut *conn, workspace_id, task.id)
        .await
        .unwrap();

    let err = get(&mut conn, workspace_id, relation.id).await.unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletes_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    let relation = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    delete(&mut conn, workspace_id, relation.id).await.unwrap();

    let err = get(&mut conn, workspace_id, relation.id).await.unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_reports_not_found_for_missing_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let err = delete(&mut conn, workspace_id, Uuid::nil())
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
    let (task, project) = seed_task_and_project(&mut conn_a, tenant_a_tenant, tenant_a).await;
    let relation = create(
        &mut conn_a,
        tenant_a,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let mut conn_b = db
        .acquire_for_workspace(tenant_b_tenant, tenant_b)
        .await
        .unwrap();
    let result = get(&mut conn_b, tenant_b, relation.id).await;
    assert!(matches!(result, Err(YorishiroError::NotFound { .. })));

    // tenant_b can't see tenant_a's entities either, so the source/target existence check itself reports NotFound.
    let cross_tenant_err = create(
        &mut conn_b,
        tenant_b,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(cross_tenant_err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn lists_relations_filtered_by_source(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let relations = list(
        &mut conn,
        workspace_id,
        ListRelationsQuery {
            source_id: Some(task.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].target_id, project.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_returns_both_directions_with_relation_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let from_task = neighbors(&mut conn, workspace_id, task.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert_eq!(from_task.len(), 1);
    assert_eq!(from_task[0].direction, "out");
    assert_eq!(from_task[0].relation_type, "belongs_to");
    assert_eq!(from_task[0].entity.id, project.id);

    let from_project = neighbors(&mut conn, workspace_id, project.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert_eq!(from_project.len(), 1);
    assert_eq!(from_project[0].direction, "in");
    assert_eq!(from_project[0].relation_type, "belongs_to");
    assert_eq!(from_project[0].entity.id, task.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_is_empty_when_no_relations_exist(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, _project) =
        seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    let result = neighbors(&mut conn, workspace_id, task.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert!(result.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_batch_matches_per_id_neighbors_calls_for_multiple_pivots(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let mut batch = neighbors_batch(
        &mut conn,
        workspace_id,
        &[task.id, project.id],
        DEFAULT_NEIGHBORS_LIMIT,
    )
    .await
    .unwrap();

    let from_task = batch.remove(&task.id).expect("task has one neighbor");
    assert_eq!(from_task.len(), 1);
    assert_eq!(from_task[0].direction, "out");
    assert_eq!(from_task[0].entity.id, project.id);

    let from_project = batch.remove(&project.id).expect("project has one neighbor");
    assert_eq!(from_project.len(), 1);
    assert_eq!(from_project[0].direction, "in");
    assert_eq!(from_project[0].entity.id, task.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_batch_omits_pivots_with_no_relations(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, _project) =
        seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    let batch = neighbors_batch(&mut conn, workspace_id, &[task.id], DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();

    assert!(batch.is_empty());
}

/// A duplicate id in `pivot_ids` must contribute its neighbors only once: `unnest` would otherwise drive the lateral subquery twice for that id and double its entry in the result.
#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_batch_dedups_a_repeated_pivot_id(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id_tenant, workspace_id).await;

    create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let batch = neighbors_batch(
        &mut conn,
        workspace_id,
        &[task.id, task.id],
        DEFAULT_NEIGHBORS_LIMIT,
    )
    .await
    .unwrap();

    let from_task = &batch[&task.id];
    assert_eq!(from_task.len(), 1);
    assert_eq!(from_task[0].entity.id, project.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_batch_applies_limit_per_pivot_not_across_the_whole_batch(pool: PgPool) {
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
        project_task_schema(),
    )
    .await
    .unwrap();

    // Two tasks, each linked to two of their own projects: if `limit` were applied across the whole batch instead of per pivot, one task's neighbors would starve the other's.
    let mut task_ids = Vec::new();
    for task_name in ["task-a", "task-b"] {
        let task = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": task_name }),
            },
            None,
        )
        .await
        .unwrap();
        for project_name in ["p1", "p2"] {
            let project = entities::create(
                &mut conn,
                workspace_id,
                entities::CreateEntityInput {
                    schema_name: "task-management".into(),
                    entity_type: "project".into(),
                    data: json!({ "title": format!("{task_name}-{project_name}") }),
                },
                None,
            )
            .await
            .unwrap();
            create(
                &mut conn,
                workspace_id,
                CreateRelationInput {
                    source_id: task.id,
                    target_id: project.id,
                    relation_type: "belongs_to".into(),
                    properties: Value::Null,
                },
            )
            .await
            .unwrap();
        }
        task_ids.push(task.id);
    }

    let batch = neighbors_batch(&mut conn, workspace_id, &task_ids, 2)
        .await
        .unwrap();

    assert_eq!(batch[&task_ids[0]].len(), 2);
    assert_eq!(batch[&task_ids[1]].len(), 2);
}

/// `neighbors_batch` groups its `CROSS JOIN LATERAL` rows into a per-pivot `Vec` in whatever order they arrive from Postgres; this pins that order to most-recent-first (matching `neighbors`' documented order) rather than leaving it as an accident of query planning, since `recall_context` relies on that order when it truncates to `limit`.
#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_batch_orders_each_pivots_neighbors_most_recent_first(pool: PgPool) {
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
        project_task_schema(),
    )
    .await
    .unwrap();

    let task = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report" }),
        },
        None,
    )
    .await
    .unwrap();

    // Created in order alpha, beta, gamma: relation_created_at is monotonically increasing, so the most-recent-first order is exactly the reverse of creation order.
    let mut project_ids = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        let project = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "project".into(),
                data: json!({ "title": name }),
            },
            None,
        )
        .await
        .unwrap();
        create(
            &mut conn,
            workspace_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();
        project_ids.push(project.id);
    }

    // limit=2 against 3 relations: truncation must drop the oldest (alpha), keeping gamma then beta.
    // That is the same outcome a single `neighbors(&mut conn, workspace_id, task.id, 2)` call would produce.
    let batch = neighbors_batch(&mut conn, workspace_id, &[task.id], 2)
        .await
        .unwrap();

    let from_task = &batch[&task.id];
    assert_eq!(from_task.len(), 2);
    assert_eq!(
        from_task[0].entity.id, project_ids[2],
        "gamma (newest) first"
    );
    assert_eq!(from_task[1].entity.id, project_ids[1], "beta second");
}

#[sqlx::test(migrations = "../../migrations")]
async fn relation_is_created_active(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, tenant_id, workspace_id).await;

    let created = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    assert_eq!(created.status, "active");
}

/// The point of the status column: a deprecated relation stops being traversed, in both directions and through both the single and the batched path, while the row itself stays.
#[sqlx::test(migrations = "../../migrations")]
async fn traversal_skips_non_active_relations(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, tenant_id, workspace_id).await;

    let relation = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let out = neighbors(&mut conn, workspace_id, task.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert_eq!(out.len(), 1, "active relation is traversed");

    let updated = set_status(&mut conn, workspace_id, relation.id, "deprecated")
        .await
        .unwrap();
    assert_eq!(updated.status, "deprecated");

    // Outbound, from the source.
    let out = neighbors(&mut conn, workspace_id, task.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert!(out.is_empty(), "deprecated relation is not traversed");

    // Inbound, from the target: the 'in' branch of the union is a separate WHERE clause and would keep returning the relation if only the 'out' branch had been filtered.
    let inbound = neighbors(&mut conn, workspace_id, project.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert!(inbound.is_empty(), "not traversed from the target either");

    // The batched path is a separate query and has to filter too.
    let batch = neighbors_batch(&mut conn, workspace_id, &[task.id], DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert!(
        batch.get(&task.id).is_none_or(|n| n.is_empty()),
        "batched traversal filters as well"
    );

    // The record survives; this is what distinguishes deprecating from deleting.
    let fetched = get(&mut conn, workspace_id, relation.id).await.unwrap();
    assert_eq!(fetched.status, "deprecated");
}

#[sqlx::test(migrations = "../../migrations")]
async fn lists_by_status_and_defaults_to_every_state(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, tenant_id, workspace_id).await;

    let relation = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();
    set_status(&mut conn, workspace_id, relation.id, "archived")
        .await
        .unwrap();

    // No status filter: an archived relation is still listed, so a caller that predates the column does not silently lose rows.
    let all = list(&mut conn, workspace_id, ListRelationsQuery::default())
        .await
        .unwrap();
    assert_eq!(all.len(), 1);

    let archived = list(
        &mut conn,
        workspace_id,
        ListRelationsQuery {
            status: Some("archived".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(archived.len(), 1);

    let active = list(
        &mut conn,
        workspace_id,
        ListRelationsQuery {
            status: Some("active".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(active.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_unknown_status(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, tenant_id, workspace_id).await;

    let relation = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    // Validated in Rust, so this is a 422 naming the field rather than the check constraint surfacing as an Internal.
    let err = set_status(&mut conn, workspace_id, relation.id, "retired")
        .await
        .unwrap_err();
    assert!(
        matches!(err, YorishiroError::ValidationFailed { .. }),
        "got {err:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn set_status_on_missing_relation_is_not_found(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let err = set_status(&mut conn, workspace_id, Uuid::nil(), "archived")
        .await
        .unwrap_err();
    assert!(
        matches!(err, YorishiroError::NotFound { .. }),
        "got {err:?}"
    );
}

/// `ListRelationsQuery::default()` is what a caller gets when it omits every filter, so the defaults are the de-facto API contract for an unfiltered list.
#[test]
fn the_default_list_query_filters_nothing_and_uses_the_documented_limit() {
    let query = ListRelationsQuery::default();

    assert!(query.source_id.is_none());
    assert!(query.target_id.is_none());
    assert!(query.relation_type.is_none());
    assert_eq!(query.limit, DEFAULT_LIST_LIMIT);
    assert_eq!(query.offset, 0);
}

/// `RelationRecord` is both written to the API and read back from a JSONL export, so it has to survive a serialize/deserialize round trip unchanged, including the free-form `properties`.
#[test]
fn a_relation_record_round_trips_through_json() {
    let record = RelationRecord {
        id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        source_id: uuid::Uuid::nil(),
        target_id: uuid::Uuid::nil(),
        relation_type: "depends_on".into(),
        properties: serde_json::json!({ "weight": 3, "note": "manual" }),
        status: "active".to_string(),
        created_at: chrono::Utc::now(),
    };

    let encoded = serde_json::to_string(&record).unwrap();
    let decoded: RelationRecord = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded.relation_type, record.relation_type);
    assert_eq!(decoded.properties, record.properties);
    assert_eq!(decoded.source_id, record.source_id);
    assert_eq!(decoded.target_id, record.target_id);
}
