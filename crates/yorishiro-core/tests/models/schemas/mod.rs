use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::models::schemas::{create_schema, create_schema_from, get_active_schema, get_by_id};
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

    let (record, diff) = create_schema(
        &mut conn,
        workspace_id_tenant,
        workspace_id,
        task_schema(false),
    )
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

    let (v1, _) = create_schema(
        &mut conn,
        workspace_id_tenant,
        workspace_id,
        task_schema(false),
    )
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

    let (v2, diff) = create_schema(
        &mut conn,
        workspace_id_tenant,
        workspace_id,
        required_priority,
    )
    .await
    .unwrap();
    assert_eq!(v2.version, 2);
    assert!(diff.is_breaking, "reasons: {:?}", diff.reasons);

    let archived = get_by_id(&mut *conn, workspace_id, v1.id).await.unwrap();
    assert_eq!(archived.status, "archived");

    let active = get_active_schema(&mut *conn, workspace_id, "task-management")
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

    let err = get_active_schema(&mut *conn, workspace_id, "does-not-exist")
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// **The isolation this change exists to provide.** Two workspaces under one tenant each get their own schema: creating one in the second workspace does not produce version 2 of the first, and neither can read the other's.
#[sqlx::test(migrations = "../../migrations")]
async fn schemas_do_not_leak_between_workspaces_of_one_tenant(pool: PgPool) {
    let tenant_id = test_support::seed_tenant(&pool, "iso-tenant").await;
    let workspace_a = test_support::seed_workspace(&pool, tenant_id, "first").await;
    let workspace_b = test_support::seed_workspace(&pool, tenant_id, "second").await;

    let db = TenantDb::new(pool.clone());

    let mut conn_a = db
        .acquire_for_workspace(tenant_id, workspace_a)
        .await
        .unwrap();
    let (a, _) = create_schema(&mut conn_a, tenant_id, workspace_a, task_schema(false))
        .await
        .unwrap();
    drop(conn_a);

    let mut conn_b = db
        .acquire_for_workspace(tenant_id, workspace_b)
        .await
        .unwrap();
    let (b, _) = create_schema(&mut conn_b, tenant_id, workspace_b, task_schema(false))
        .await
        .unwrap();

    // Same name, but each workspace starts its own version 1: the second is not a new version of the first.
    assert_eq!(a.version, 1);
    assert_eq!(b.version, 1);
    assert_ne!(a.id, b.id);

    // B cannot read A's schema.
    assert!(get_by_id(&mut *conn_b, workspace_b, a.id).await.is_err());

    // And B's listing shows only its own.
    let listed = crate::models::schemas::list(&mut *conn_b, workspace_b)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, b.id);
}

/// A schema written by hand claims no origin.
/// "detached" here means never linked, not orphaned: told apart by origin_template_id having never been set.
#[sqlx::test(migrations = "../../migrations")]
async fn a_hand_written_schema_has_no_origin(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let (schema, _) = create_schema(&mut conn, tenant_id, workspace_id, task_schema(false))
        .await
        .unwrap();

    assert!(schema.origin_template_id.is_none());
    assert_eq!(schema.origin_status, "detached");
}

/// Created from a library template, the schema records which one and says it is following it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_schema_from_a_template_records_its_origin(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let (schema, _) = create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();

    assert_eq!(schema.origin_template_id, Some(template_id));
    assert_eq!(schema.origin_status, "linked");
}

/// The yank: deleting the template must not destroy the copy, and must stop it claiming to follow something that is no longer there.
/// Enforced by a trigger, so a delete arriving from the admin CLI or a migration is covered too.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_the_template_detaches_the_schema_without_destroying_it(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let (schema, _) = create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();

    sqlx::query("DELETE FROM identity.templates WHERE id = $1")
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();

    let after = get_by_id(&mut *conn, workspace_id, schema.id)
        .await
        .unwrap();

    // The definition survives: this is the whole point of copying rather than referencing.
    assert_eq!(after.definition.name, schema.definition.name);
    // And it no longer claims to be following anything.
    assert!(after.origin_template_id.is_none());
    assert_eq!(
        after.origin_status, "detached",
        "a schema must not stay 'linked' with no template to link to"
    );
}

async fn seed_template(pool: &PgPool, tenant_id: Uuid) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO identity.templates (tenant_id, name, definition) \
         VALUES ($1, 'seeded', $2) RETURNING id",
    )
    .bind(tenant_id)
    .bind(serde_json::to_value(task_schema(false)).unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    id
}

/// The signal: a template edited after the copy was taken shows up as available.
#[sqlx::test(migrations = "../../migrations")]
async fn a_copy_keeps_the_definition_it_was_made_from(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let (schema, _) = create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();

    let base = schema
        .origin_snapshot
        .as_ref()
        .expect("a copy from a template has a merge base");
    assert_eq!(base.name, "task-management");
    assert!(
        !base.entity_types["task"].fields.contains_key("priority"),
        "the base is the definition as copied"
    );

    // The template moves on.
    sqlx::query("UPDATE identity.templates SET definition = $2 WHERE id = $1")
        .bind(template_id)
        .bind(serde_json::to_value(task_schema(true)).unwrap())
        .execute(&pool)
        .await
        .unwrap();

    // The base does not.
    // This is what lets a merge tell an upstream addition from a local one.
    let after = get_by_id(&mut *conn, workspace_id, schema.id)
        .await
        .unwrap();
    let base = after.origin_snapshot.expect("still there");
    assert!(
        !base.entity_types["task"].fields.contains_key("priority"),
        "the merge base must not follow the template"
    );
}

/// No origin, no ancestor.
/// A fabricated base would be worse than none, since a merge would treat every field as agreed-upon history.
#[sqlx::test(migrations = "../../migrations")]
async fn creating_a_schema_names_it_on_the_workspace(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let named: Option<Uuid> =
        sqlx::query_scalar("SELECT schema_id FROM identity.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(named.is_none(), "a fresh workspace names no schema");

    let (first, _) = create_schema(&mut conn, tenant_id, workspace_id, task_schema(false))
        .await
        .unwrap();

    let named: Option<Uuid> =
        sqlx::query_scalar("SELECT schema_id FROM identity.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(named, Some(first.id), "the first schema is the one named");

    // A second version of the same schema, and then a differently-named one: neither takes the column.
    // It means "this workspace's schema", not "the most recent one".
    let (second, _) = create_schema(&mut conn, tenant_id, workspace_id, task_schema(true))
        .await
        .unwrap();
    assert_ne!(second.id, first.id);

    let named: Option<Uuid> =
        sqlx::query_scalar("SELECT schema_id FROM identity.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        named,
        Some(first.id),
        "a later schema does not take the column from the first"
    );
}

/// The request path runs as `yorishiro_app`, and creating a schema writes `identity.workspaces`.
///
/// Every other test here connects as the owner, where grants are implicit: which is why a missing `GRANT UPDATE` on that table shipped and made `POST /api/schemas` answer 500 on both editions while the whole suite stayed green.
/// This one issues `SET ROLE` first, so it fails the way the server does.
#[sqlx::test(migrations = "../../migrations")]
async fn creating_a_schema_works_as_the_request_role(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    sqlx::query("SET ROLE yorishiro_app")
        .execute(&mut *conn)
        .await
        .unwrap();

    create_schema(&mut conn, tenant_id, workspace_id, task_schema(false))
        .await
        .expect("the request role can create a schema");
}
