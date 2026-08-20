use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::tests::test_helpers;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::TenantDb;
use yorishiro_core::metaschema::MetaSchemaDefinition;
use yorishiro_core::models::schemas::{
    create_schema, create_schema_from, create_schema_with_base, get_active_schema, get_by_id,
};

use crate::models::origin::list_with_upstream_changes;
use crate::services::origin::{merge_apply, merge_preview};

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

#[sqlx::test(migrations = "../../../migrations")]
async fn an_edited_template_is_reported_as_an_upstream_change(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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
#[sqlx::test(migrations = "../../../migrations")]
async fn a_detached_schema_is_never_an_upstream_change(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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

/// Once the template is deleted there is no update left to take, so a yanked schema drops out of the report rather than sitting in it forever.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_yanked_schema_stops_being_reported(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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

/// The merge base.
/// A copy keeps the definition it was made from, and that snapshot does not move when the template does, otherwise there would be nothing to compare the upstream edit against.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_hand_written_schema_has_no_merge_base(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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

/// The whole point, end to end: a template that moved and a workspace that moved, told apart by the base rather than confused with each other.
#[sqlx::test(migrations = "../../../migrations")]
async fn merge_preview_separates_upstream_changes_from_local_ones(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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
    assert_eq!(
        priority.verdict,
        crate::services::merge::MergeVerdict::AutoAdd
    );
    assert!(!plan.has_conflicts());
}

/// A schema that follows nothing cannot be merged, and says so rather than comparing against something arbitrary.
#[sqlx::test(migrations = "../../../migrations")]
async fn merge_preview_refuses_a_schema_with_no_origin(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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

/// Copied before snapshots existed: no ancestor, so no merge.
/// Substituting the current template would read every local field as a conflict, which is worse than refusing.
#[sqlx::test(migrations = "../../../migrations")]
async fn merge_preview_refuses_when_the_base_was_never_recorded(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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

#[sqlx::test(migrations = "../../../migrations")]
async fn merge_apply_takes_upstream_and_keeps_local(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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

    let active = get_active_schema(&mut *conn, workspace_id, "task-management")
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
    let previous = get_by_id(&mut *conn, workspace_id, active.id)
        .await
        .unwrap();
    assert_eq!(previous.status, "archived");
    assert_eq!(schema.version, 1);
}

/// The property the whole three-way apparatus rests on.
/// After a merge the base must be what upstream said, not what the merge produced, otherwise the *next* merge reads this workspace's own fields as upstream's, sees them "unchanged here", and follows a later upstream removal by deleting them.
#[sqlx::test(migrations = "../../../migrations")]
async fn merge_apply_advances_the_base_to_upstream(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    // The copy already carries a field of its own, so it must say what the template said, otherwise the base would claim `assignee` came from upstream.
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

    let active = get_active_schema(&mut *conn, workspace_id, "task-management")
        .await
        .unwrap();
    let (merged, _) = merge_apply(&mut conn, &pool, tenant_id, workspace_id, active.id)
        .await
        .unwrap();

    let base = merged.origin_snapshot.expect("a merge records its base");
    // By serialised form, as `merge::same` compares: unknown `x-` attributes ride in a flattened map that a field-by-field comparison would not see.
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
    assert_eq!(
        assignee.verdict,
        crate::services::merge::MergeVerdict::KeepLocal
    );
}

#[sqlx::test(migrations = "../../../migrations")]
async fn merge_apply_refuses_a_conflict(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
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

    let active = get_active_schema(&mut *conn, workspace_id, "task-management")
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
    let still = get_active_schema(&mut *conn, workspace_id, "task-management")
        .await
        .unwrap();
    assert_eq!(still.id, active.id);
}

/// `get_by_id` returns any version, archived included: that is how a caller reads an old definition.
/// Merging into one is different: the merge installs its result as the new active version, so an archived id would resurrect an abandoned lineage over the one entities are actually written against.
/// Both preview and apply refuse, since they share `merge_sides`.
#[sqlx::test(migrations = "../../../migrations")]
async fn an_archived_version_cannot_be_merged_into(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let template_id = seed_template(&pool, tenant_id).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let (first, _) = create_schema_from(
        &mut conn,
        tenant_id,
        workspace_id,
        task_schema(false),
        Some(template_id),
    )
    .await
    .unwrap();

    // A second version archives the first.
    create_schema(&mut conn, tenant_id, workspace_id, task_schema(true))
        .await
        .unwrap();
    let archived = get_by_id(&mut *conn, workspace_id, first.id).await.unwrap();
    assert_eq!(
        archived.status, "archived",
        "the first version must be archived"
    );

    for result in [
        merge_preview(&mut conn, &pool, tenant_id, workspace_id, first.id)
            .await
            .err(),
        merge_apply(&mut conn, &pool, tenant_id, workspace_id, first.id)
            .await
            .err(),
    ] {
        assert!(
            matches!(result, Some(YorishiroError::ValidationFailed { ref message, .. }) if message.contains("archived")),
            "an archived version must be refused, got {result:?}"
        );
    }
}
