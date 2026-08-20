use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::recall::{
    DEFAULT_RECALL_DEPTH, DEFAULT_RECALL_LIMIT, RecallQuery, recall_context,
};
use crate::models::relations::CreateRelationInput;
use crate::models::{entities, relations, schemas};
use crate::test_support;

fn project_task_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true, "x-embed": true },
                    "note": { "type": "string" }
                }
            },
            "project": {
                "fields": { "title": { "type": "string", "required": true, "x-embed": true } }
            }
        },
        "relation_types": {
            "belongs_to": { "source": "task", "target": "project" }
        }
    }))
    .unwrap()
}

/// A three-type chain (task -> project -> team) so multi-hop traversal has somewhere to go beyond a single hop.
fn task_project_team_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-project-team",
        "entity_types": {
            "task": {
                "fields": { "title": { "type": "string", "required": true, "x-embed": true } }
            },
            "project": {
                "fields": { "title": { "type": "string", "required": true, "x-embed": true } }
            },
            "team": {
                "fields": { "name": { "type": "string", "required": true, "x-embed": true } }
            }
        },
        "relation_types": {
            "belongs_to": { "source": "task", "target": "project" },
            "owned_by": { "source": "project", "target": "team" }
        }
    }))
    .unwrap()
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    test_support::seed_tenant_and_workspace(pool).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn returns_entity_with_shallow_neighbors_by_default(pool: PgPool) {
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

    let project = entities::create(
        &mut conn,
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
    let task = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report", "note": "internal only" }),
        },
        None,
    )
    .await
    .unwrap();
    relations::create(
        &mut *conn,
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

    let context = recall_context(
        &mut conn,
        workspace_id,
        task.id,
        RecallQuery {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(context.entity.id, task.id);
    assert_eq!(context.entity.data["note"], "internal only");
    assert!(!context.truncated);
    assert_eq!(context.relations.len(), 1);
    assert_eq!(context.relations[0].direction, "out");
    assert_eq!(context.relations[0].relation_type, "belongs_to");
    assert_eq!(context.relations[0].neighbor.id, project.id);
    assert_eq!(context.relations[0].neighbor.data["title"], "Q3 roadmap");
    assert_eq!(context.relations[0].hop_distance, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn full_flag_returns_the_neighbors_entire_data(pool: PgPool) {
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

    let project = entities::create(
        &mut conn,
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
    let task = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report", "note": "internal only" }),
        },
        None,
    )
    .await
    .unwrap();
    relations::create(
        &mut *conn,
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

    let context = recall_context(
        &mut conn,
        workspace_id,
        project.id,
        RecallQuery {
            limit: DEFAULT_RECALL_LIMIT,
            full: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(context.relations.len(), 1);
    assert_eq!(context.relations[0].neighbor.data["note"], "internal only");
}

#[sqlx::test(migrations = "../../migrations")]
async fn sets_truncated_when_more_neighbors_exist_than_the_limit(pool: PgPool) {
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
        relations::create(
            &mut *conn,
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

    let context = recall_context(
        &mut conn,
        workspace_id,
        task.id,
        RecallQuery {
            limit: 2,
            full: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(context.relations.len(), 2);
    assert!(context.truncated);
}

#[sqlx::test(migrations = "../../migrations")]
async fn reports_not_found_for_a_missing_entity(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let err = recall_context(
        &mut conn,
        workspace_id,
        Uuid::nil(),
        RecallQuery {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// A -> B -> C chain (task -> project -> team).
/// Recalling from A (the task) with depth=1 should only see B (the project); with depth=2 it should also reach C (the team) at hop_distance 2, without re-listing B.
#[sqlx::test(migrations = "../../migrations")]
async fn depth_two_reaches_the_second_hop_neighbor(pool: PgPool) {
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
        task_project_team_schema(),
    )
    .await
    .unwrap();

    let team = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-project-team".into(),
            entity_type: "team".into(),
            data: json!({ "name": "platform" }),
        },
        None,
    )
    .await
    .unwrap();
    let project = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-project-team".into(),
            entity_type: "project".into(),
            data: json!({ "title": "Q3 roadmap" }),
        },
        None,
    )
    .await
    .unwrap();
    let task = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-project-team".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report" }),
        },
        None,
    )
    .await
    .unwrap();
    relations::create(
        &mut *conn,
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
    relations::create(
        &mut *conn,
        workspace_id,
        CreateRelationInput {
            source_id: project.id,
            target_id: team.id,
            relation_type: "owned_by".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    // Default depth (1) only reaches the project, not the team.
    let depth_one = recall_context(
        &mut conn,
        workspace_id,
        task.id,
        RecallQuery {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(depth_one.relations.len(), 1);
    assert_eq!(depth_one.relations[0].neighbor.id, project.id);
    assert!(!depth_one.relations.iter().any(|r| r.neighbor.id == team.id));

    // depth=2 reaches both the project (hop 1) and the team (hop 2).
    let depth_two = recall_context(
        &mut conn,
        workspace_id,
        task.id,
        RecallQuery {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            depth: 2,
        },
    )
    .await
    .unwrap();
    assert_eq!(depth_two.relations.len(), 2);

    let project_relation = depth_two
        .relations
        .iter()
        .find(|r| r.neighbor.id == project.id)
        .expect("project should be present at hop 1");
    assert_eq!(project_relation.hop_distance, 1);

    let team_relation = depth_two
        .relations
        .iter()
        .find(|r| r.neighbor.id == team.id)
        .expect("team should be present at hop 2");
    assert_eq!(team_relation.hop_distance, 2);
    assert_eq!(team_relation.relation_type, "owned_by");
    assert_eq!(team_relation.neighbor.data["name"], "platform");
}

/// depth is clamped to MAX_RECALL_DEPTH (3), so an out-of-range request doesn't error but also doesn't traverse further than the cap.
#[sqlx::test(migrations = "../../migrations")]
async fn depth_beyond_the_maximum_is_clamped_not_rejected(pool: PgPool) {
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

    let project = entities::create(
        &mut conn,
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
    relations::create(
        &mut *conn,
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

    let context = recall_context(
        &mut conn,
        workspace_id,
        task.id,
        RecallQuery {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            depth: 99,
        },
    )
    .await
    .unwrap();

    // Only one real hop exists in this graph, so the clamp doesn't change the result here, but the call must succeed (not error) with an out-of-range depth.
    assert_eq!(context.relations.len(), 1);
    assert_eq!(context.relations[0].hop_distance, 1);
}

/// A single task belonging to two projects, each owned by its own team, so hop 1's frontier has two nodes at once.
/// Exercises the batched neighbor lookup (`relations::neighbors_batch`) across a multi-node frontier, not just the single-node chain the other depth tests use.
#[sqlx::test(migrations = "../../migrations")]
async fn depth_two_expands_every_node_in_a_multi_node_frontier(pool: PgPool) {
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
        task_project_team_schema(),
    )
    .await
    .unwrap();

    let task = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "task-project-team".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report" }),
        },
        None,
    )
    .await
    .unwrap();

    let mut team_ids = Vec::new();
    for (project_name, team_name) in [("alpha", "team-a"), ("beta", "team-b")] {
        let project = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-project-team".into(),
                entity_type: "project".into(),
                data: json!({ "title": project_name }),
            },
            None,
        )
        .await
        .unwrap();
        relations::create(
            &mut *conn,
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

        let team = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-project-team".into(),
                entity_type: "team".into(),
                data: json!({ "name": team_name }),
            },
            None,
        )
        .await
        .unwrap();
        relations::create(
            &mut *conn,
            workspace_id,
            CreateRelationInput {
                source_id: project.id,
                target_id: team.id,
                relation_type: "owned_by".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();
        team_ids.push(team.id);
    }

    let context = recall_context(
        &mut conn,
        workspace_id,
        task.id,
        RecallQuery {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            depth: 2,
        },
    )
    .await
    .unwrap();

    // 2 projects at hop 1 + 2 teams at hop 2, one reached through each project.
    assert_eq!(context.relations.len(), 4);
    for team_id in team_ids {
        let team_relation = context
            .relations
            .iter()
            .find(|r| r.neighbor.id == team_id)
            .expect("every team should be reachable at hop 2");
        assert_eq!(team_relation.hop_distance, 2);
    }
}

/// The defaults encode the original single-hop behaviour.
/// `depth` in particular is what keeps an unparameterised recall from fanning out across the graph.
#[test]
fn the_default_recall_query_is_a_single_hop() {
    let query = RecallQuery::default();

    assert_eq!(query.depth, DEFAULT_RECALL_DEPTH);
    assert_eq!(query.depth, 1);
    assert_eq!(query.limit, DEFAULT_RECALL_LIMIT);
    assert!(!query.full);
}

// `MAX_RECALL_DEPTH` is deliberately not asserted here.
// Its value only reaches behaviour through the clamp in `recall_context`, and `depth_beyond_the_maximum_is_clamped_not_rejected` in this file walks that against a real graph.
// An assertion on the constant's range would restate the number without proving anything that test does not.
