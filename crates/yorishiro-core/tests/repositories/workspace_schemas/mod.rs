use serde_json::json;
use sqlx::PgPool;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::repositories::{schemas, workspace_schemas};
use crate::test_support;

fn task_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string", "required": true } } }
        }
    }))
    .unwrap()
}

/// The same schema with an extra field, standing in for "the tenant edited its schema".
fn task_schema_v2() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "priority": { "type": "string", "required": false }
                }
            }
        }
    }))
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_workspace_has_no_fork_until_it_forks(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    assert!(
        workspace_schemas::get(&mut conn, workspace_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn forking_copies_the_tenants_active_definition(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let (source, _) = schemas::create_schema(&mut conn, tenant_id, task_schema())
        .await
        .unwrap();

    let fork = workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap();

    assert_eq!(fork.source_id, source.id);
    assert_eq!(fork.source_version, source.version);
    assert!(!fork.customized, "a fresh fork is an unmodified copy");
    assert!(fork.definition.entity_types.contains_key("task"));
}

/// A workspace has one schema, so a second fork would leave "which one is this workspace's"
/// unanswerable.
#[sqlx::test(migrations = "../../migrations")]
async fn a_workspace_cannot_fork_twice(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema())
        .await
        .unwrap();
    workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap();

    let err = workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap_err();

    assert!(matches!(err, YorishiroError::Conflict { .. }));
}

/// The point of the whole feature: an edit to the fork stays inside the workspace.
#[sqlx::test(migrations = "../../migrations")]
async fn editing_a_fork_does_not_touch_the_tenants_schema(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema())
        .await
        .unwrap();
    workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap();

    let edited = workspace_schemas::update_definition(&mut conn, workspace_id, task_schema_v2())
        .await
        .unwrap();
    assert!(edited.customized);
    assert!(
        edited.definition.entity_types["task"]
            .fields
            .contains_key("priority")
    );

    // The tenant's own schema is untouched -- still one version, still without the field.
    let tenant_schema = schemas::get_active_schema(&mut conn, tenant_id, "task-management")
        .await
        .unwrap();
    assert_eq!(tenant_schema.version, 1);
    assert!(
        !tenant_schema.definition.entity_types["task"]
            .fields
            .contains_key("priority")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_fork_reports_when_the_tenants_schema_has_moved_on(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema())
        .await
        .unwrap();
    let fork = workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap();

    // Nothing has changed upstream yet.
    assert_eq!(
        workspace_schemas::upstream_version(&mut conn, tenant_id, &fork)
            .await
            .unwrap(),
        None
    );

    schemas::create_schema(&mut conn, tenant_id, task_schema_v2())
        .await
        .unwrap();

    assert_eq!(
        workspace_schemas::upstream_version(&mut conn, tenant_id, &fork)
            .await
            .unwrap(),
        Some(2)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_untouched_fork_follows_the_tenants_schema(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema())
        .await
        .unwrap();
    workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema_v2())
        .await
        .unwrap();

    // No force needed: there are no local edits to discard.
    let followed = workspace_schemas::follow_upstream(&mut conn, tenant_id, workspace_id, false)
        .await
        .unwrap();

    assert_eq!(followed.source_version, 2);
    assert!(!followed.customized);
    assert!(
        followed.definition.entity_types["task"]
            .fields
            .contains_key("priority")
    );
}

/// **Following must not silently discard someone's schema work.** A customized fork refuses
/// until the caller says explicitly that overwriting is what they want.
#[sqlx::test(migrations = "../../migrations")]
async fn a_customized_fork_refuses_to_follow_without_force(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema())
        .await
        .unwrap();
    workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap();
    workspace_schemas::update_definition(&mut conn, workspace_id, task_schema_v2())
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema_v2())
        .await
        .unwrap();

    let err = workspace_schemas::follow_upstream(&mut conn, tenant_id, workspace_id, false)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::Conflict { .. }));

    // With force, the local edits are replaced and the fork is an unmodified copy again.
    let followed = workspace_schemas::follow_upstream(&mut conn, tenant_id, workspace_id, true)
        .await
        .unwrap();
    assert!(!followed.customized);
    assert_eq!(followed.source_version, 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn unforking_returns_the_workspace_to_its_tenants_schema(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant_id, task_schema())
        .await
        .unwrap();
    workspace_schemas::fork(&mut conn, tenant_id, workspace_id, "task-management")
        .await
        .unwrap();

    workspace_schemas::unfork(&mut conn, workspace_id)
        .await
        .unwrap();

    assert!(
        workspace_schemas::get(&mut conn, workspace_id)
            .await
            .unwrap()
            .is_none()
    );

    // Unforking twice is a NotFound, not a silent success.
    let err = workspace_schemas::unfork(&mut conn, workspace_id)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// One workspace's fork must not be visible to another, the same way entities are not.
#[sqlx::test(migrations = "../../migrations")]
async fn a_fork_is_invisible_to_another_workspace(pool: PgPool) {
    let (tenant_id, workspace_a) = test_support::seed_tenant_and_workspace(&pool).await;
    let workspace_b = test_support::seed_workspace(&pool, tenant_id, "second-workspace").await;
    let db = TenantDb::new(pool);

    let mut conn_a = db
        .acquire_for_workspace(tenant_id, workspace_a)
        .await
        .unwrap();
    schemas::create_schema(&mut conn_a, tenant_id, task_schema())
        .await
        .unwrap();
    workspace_schemas::fork(&mut conn_a, tenant_id, workspace_a, "task-management")
        .await
        .unwrap();
    drop(conn_a);

    let mut conn_b = db
        .acquire_for_workspace(tenant_id, workspace_b)
        .await
        .unwrap();
    assert!(
        workspace_schemas::get(&mut conn_b, workspace_b)
            .await
            .unwrap()
            .is_none()
    );
}
