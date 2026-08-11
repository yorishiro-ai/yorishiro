use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::repositories::schemas::{
    create_schema, create_schema_from, create_schema_with_base, get_active_schema, get_by_id,
    list_with_upstream_changes, merge_apply, merge_preview,
};
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

    let archived = get_by_id(&mut conn, workspace_id, v1.id).await.unwrap();
    assert_eq!(archived.status, "archived");

    let active = get_active_schema(&mut conn, workspace_id, "task-management")
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

    let err = get_active_schema(&mut conn, workspace_id, "does-not-exist")
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// **The isolation this change exists to provide.** Two workspaces under one tenant each get
/// their own schema: creating one in the second workspace does not produce version 2 of the
/// first, and neither can read the other's.
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

    // Same name, but each workspace starts its own version 1 -- the second is not a new
    // version of the first.
    assert_eq!(a.version, 1);
    assert_eq!(b.version, 1);
    assert_ne!(a.id, b.id);

    // B cannot read A's schema.
    assert!(get_by_id(&mut conn_b, workspace_b, a.id).await.is_err());

    // And B's listing shows only its own.
    let listed = crate::repositories::schemas::list(&mut conn_b, workspace_b)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, b.id);
}

/// A schema written by hand claims no origin. "detached" here means never linked, not
/// orphaned — told apart by origin_template_id having never been set.
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

/// The yank: deleting the template must not destroy the copy, and must stop it claiming to
/// follow something that is no longer there. Enforced by a trigger, so a delete arriving from
/// the admin CLI or a migration is covered too.
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

    let after = get_by_id(&mut conn, workspace_id, schema.id).await.unwrap();

    // The definition survives -- this is the whole point of copying rather than referencing.
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
async fn an_edited_template_is_reported_as_an_upstream_change(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();

    // Nothing has changed upstream yet.
    let changes = list_with_upstream_changes(&pool, workspace_id)
        .await
        .unwrap();
    assert!(changes.is_empty(), "an untouched template is not a change");

    // The tenant admin edits the template.
    sqlx::query(
        "UPDATE identity.templates SET updated_at = now() + interval '1 second' WHERE id = $1",
    )
    .bind(template_id)
    .execute(&pool)
    .await
    .unwrap();

    let changes = list_with_upstream_changes(&pool, workspace_id)
        .await
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].template_id, template_id);
    assert_eq!(changes[0].schema_name, "task-management");
}

/// A schema written by hand follows nothing, so it can never be reported.
#[sqlx::test(migrations = "../../migrations")]
async fn a_detached_schema_is_never_an_upstream_change(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    create_schema(&mut conn, tenant_id, workspace_id, task_schema(false))
        .await
        .unwrap();

    let changes = list_with_upstream_changes(&pool, workspace_id)
        .await
        .unwrap();
    assert!(changes.is_empty());
}

/// Once the template is deleted there is no update left to take, so a yanked schema drops out
/// of the report rather than sitting in it forever.
#[sqlx::test(migrations = "../../migrations")]
async fn a_yanked_schema_stops_being_reported(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE identity.templates SET updated_at = now() + interval '1 second' WHERE id = $1",
    )
    .bind(template_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        list_with_upstream_changes(&pool, workspace_id)
            .await
            .unwrap()
            .len(),
        1
    );

    sqlx::query("DELETE FROM identity.templates WHERE id = $1")
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();

    let changes = list_with_upstream_changes(&pool, workspace_id)
        .await
        .unwrap();
    assert!(changes.is_empty(), "a yanked schema has no update to take");
}

/// The merge base. A copy keeps the definition it was made from, and that snapshot does not
/// move when the template does — otherwise there would be nothing to compare the upstream
/// edit against.
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

    // The base does not. This is what lets a merge tell an upstream addition from a local one.
    let after = get_by_id(&mut conn, workspace_id, schema.id).await.unwrap();
    let base = after.origin_snapshot.expect("still there");
    assert!(
        !base.entity_types["task"].fields.contains_key("priority"),
        "the merge base must not follow the template"
    );
}

/// No origin, no ancestor. A fabricated base would be worse than none, since a merge would
/// treat every field as agreed-upon history.
#[sqlx::test(migrations = "../../migrations")]
async fn a_hand_written_schema_has_no_merge_base(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let (schema, _) = create_schema(&mut conn, tenant_id, workspace_id, task_schema(false))
        .await
        .unwrap();

    assert!(schema.origin_snapshot.is_none());
}

/// The whole point, end to end: a template that moved and a workspace that moved, told apart
/// by the base rather than confused with each other.
#[sqlx::test(migrations = "../../migrations")]
async fn merge_preview_separates_upstream_changes_from_local_ones(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    // Copy the template as it stands.
    let (schema, _) = create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();

    // Upstream adds `priority`.
    sqlx::query("UPDATE identity.templates SET definition = $2 WHERE id = $1")
        .bind(template_id)
        .bind(serde_json::to_value(task_schema(true)).unwrap())
        .execute(&pool)
        .await
        .unwrap();

    let plan = merge_preview(&mut conn, &pool, tenant_id, workspace_id, schema.id)
        .await
        .unwrap();

    let priority = plan
        .fields
        .iter()
        .find(|f| f.field == "priority")
        .expect("the upstream addition is reported");
    assert_eq!(priority.verdict, crate::metaschema::MergeVerdict::AutoAdd);
    assert!(!plan.has_conflicts());
}

/// A schema that follows nothing cannot be merged, and says so rather than comparing against
/// something arbitrary.
#[sqlx::test(migrations = "../../migrations")]
async fn merge_preview_refuses_a_schema_with_no_origin(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let (schema, _) = create_schema(&mut conn, tenant_id, workspace_id, task_schema(false))
        .await
        .unwrap();

    let err = merge_preview(&mut conn, &pool, tenant_id, workspace_id, schema.id)
        .await
        .unwrap_err();

    assert!(
        matches!(err, YorishiroError::ValidationFailed { .. }),
        "got {err:?}"
    );
}

/// Copied before snapshots existed: no ancestor, so no merge. Substituting the current
/// template would read every local field as a conflict, which is worse than refusing.
#[sqlx::test(migrations = "../../migrations")]
async fn merge_preview_refuses_when_the_base_was_never_recorded(pool: PgPool) {
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

    // A row as it would look if copied before the snapshot column existed.
    sqlx::query("UPDATE content.schemas SET origin_snapshot = NULL WHERE id = $1")
        .bind(schema.id)
        .execute(&pool)
        .await
        .unwrap();

    let err = merge_preview(&mut conn, &pool, tenant_id, workspace_id, schema.id)
        .await
        .unwrap_err();

    assert!(
        matches!(err, YorishiroError::ValidationFailed { .. }),
        "got {err:?}"
    );
}

/// A schema with a field of its own, on top of the template's.
fn task_schema_with_local_field() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": { "task": { "fields": {
            "title": { "type": "string", "required": true },
            "assignee": { "type": "string" }
        } } }
    }))
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn merge_apply_takes_upstream_and_keeps_local(pool: PgPool) {
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

    // This workspace adds `assignee`; the template knows nothing about it.
    create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema_with_local_field(),
        Some(template_id),
    )
    .await
    .unwrap();

    // Upstream adds `priority`.
    sqlx::query("UPDATE identity.templates SET definition = $2 WHERE id = $1")
        .bind(template_id)
        .bind(serde_json::to_value(task_schema(true)).unwrap())
        .execute(&pool)
        .await
        .unwrap();

    let active = get_active_schema(&mut conn, workspace_id, "task-management")
        .await
        .unwrap();
    let (merged, _) = merge_apply(&mut conn, &pool, tenant_id, workspace_id, active.id)
        .await
        .unwrap();

    let fields = &merged.definition.entity_types["task"].fields;
    assert!(
        fields.contains_key("priority"),
        "upstream's addition is taken"
    );
    assert!(
        fields.contains_key("assignee"),
        "the local addition survives"
    );
    assert_eq!(merged.version, active.version + 1);
    assert_eq!(merged.status, "active");
    // Superseded, not deleted: entities written against it still validate against it.
    let previous = get_by_id(&mut conn, workspace_id, active.id).await.unwrap();
    assert_eq!(previous.status, "archived");
    assert_eq!(schema.version, 1);
}

/// The property the whole three-way apparatus rests on. After a merge the base must be what
/// upstream said, not what the merge produced -- otherwise the *next* merge reads this
/// workspace's own fields as upstream's, sees them "unchanged here", and follows a later
/// upstream removal by deleting them.
#[sqlx::test(migrations = "../../migrations")]
async fn merge_apply_advances_the_base_to_upstream(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    // The copy already carries a field of its own, so it must say what the template said --
    // otherwise the base would claim `assignee` came from upstream.
    create_schema_with_base(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema_with_local_field(),
        Some(template_id),
        Some(task_schema(false)),
    )
    .await
    .unwrap();

    sqlx::query("UPDATE identity.templates SET definition = $2 WHERE id = $1")
        .bind(template_id)
        .bind(serde_json::to_value(task_schema(true)).unwrap())
        .execute(&pool)
        .await
        .unwrap();

    let active = get_active_schema(&mut conn, workspace_id, "task-management")
        .await
        .unwrap();
    let (merged, _) = merge_apply(&mut conn, &pool, tenant_id, workspace_id, active.id)
        .await
        .unwrap();

    let base = merged.origin_snapshot.expect("a merge records its base");
    // By serialised form, as `merge::same` compares: unknown `x-` attributes ride in a
    // flattened map that a field-by-field comparison would not see.
    assert_eq!(
        serde_json::to_value(&base).unwrap(),
        serde_json::to_value(task_schema(true)).unwrap(),
        "the base is upstream at merge time, not the merged result"
    );

    // And so the next merge still sees `assignee` as this workspace's own.
    let plan = merge_preview(&mut conn, &pool, tenant_id, workspace_id, merged.id)
        .await
        .unwrap();
    let assignee = plan
        .fields
        .iter()
        .find(|f| f.field == "assignee")
        .expect("the local field is still local");
    assert_eq!(assignee.verdict, crate::metaschema::MergeVerdict::KeepLocal);
}

#[sqlx::test(migrations = "../../migrations")]
async fn merge_apply_refuses_a_conflict(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();

    // Both sides add `priority`, with different types.
    let local: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": { "task": { "fields": {
            "title": { "type": "string", "required": true },
            "priority": { "type": "string" }
        } } }
    }))
    .unwrap();
    create_schema_from(&mut conn, tenant_id, workspace_id, local, Some(template_id))
        .await
        .unwrap();

    sqlx::query("UPDATE identity.templates SET definition = $2 WHERE id = $1")
        .bind(template_id)
        .bind(serde_json::to_value(task_schema(true)).unwrap())
        .execute(&pool)
        .await
        .unwrap();

    let active = get_active_schema(&mut conn, workspace_id, "task-management")
        .await
        .unwrap();
    let err = merge_apply(&mut conn, &pool, tenant_id, workspace_id, active.id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, YorishiroError::ValidationFailed { .. }),
        "got {err:?}"
    );

    // Nothing written: the active version is the one that was active before.
    let still = get_active_schema(&mut conn, workspace_id, "task-management")
        .await
        .unwrap();
    assert_eq!(still.id, active.id);
}

/// `identity.workspaces.schema_id` names the workspace's schema, and the workspace listing shows
/// it. Creating a schema used to leave it NULL — the column was only ever written by the admin
/// CLI — so a workspace provisioned over REST or MCP listed no schema at all.
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

    // A second version of the same schema, and then a differently-named one: neither takes the
    // column. It means "this workspace's schema", not "the most recent one".
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
/// Every other test here connects as the owner, where grants are implicit — which is why a
/// missing `GRANT UPDATE` on that table shipped and made `POST /api/schemas` answer 500 on both
/// editions while the whole suite stayed green. This one issues `SET ROLE` first, so it fails
/// the way the server does.
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
