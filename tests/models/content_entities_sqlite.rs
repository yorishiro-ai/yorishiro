use migration::{Migrator, MigratorTrait};
/// SQLite-specific tests for content_entities: CRUD operations and snapshot undo.
use sea_orm::{ConnectionTrait, Database, Statement};
use yorishiro::models::content_entities::{self, CreateEntityInput, ListEntitiesQuery};

/// A fresh in-memory SQLite database, migrated, with one tenant/workspace/schema seeded via raw SQL (not through `tenancy`/`content_schemas`, to keep this test focused on `content_entities` itself).
/// Mirrors `tenancy.rs`'s own `sqlite_db()` test helper.
async fn seeded_sqlite_db() -> (sea_orm::DatabaseConnection, uuid::Uuid) {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");

    let tenant_id = uuid::Uuid::now_v7();
    let workspace_id = uuid::Uuid::now_v7();
    let schema_id = uuid::Uuid::now_v7();

    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO identity_tenants (id, name) VALUES ($1, 'acme')",
        [tenant_id.into()],
    ))
    .await
    .expect("insert tenant");

    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO identity_workspaces (id, tenant_id, name, status, max_entities) \
         VALUES ($1, $2, 'ws', 'active', NULL)",
        [workspace_id.into(), tenant_id.into()],
    ))
    .await
    .expect("insert workspace");

    let definition = serde_json::json!({
        "name": "notes",
        "entity_types": {
            "note": { "fields": {}, "required": [] }
        }
    });

    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO content_schemas \
            (id, tenant_id, workspace_id, name, version, definition, status) \
         VALUES ($1, $2, $3, 'notes', 1, $4, 'active')",
        [
            schema_id.into(),
            tenant_id.into(),
            workspace_id.into(),
            definition.to_string().into(),
        ],
    ))
    .await
    .expect("insert schema");

    (db, workspace_id)
}

/// Exercises all eight query functions against SQLite in one pass: `count`, `get`, `get_batch`,
/// `list`, `export_all`, `create`, `update` and `delete`.
#[tokio::test]
async fn content_entities_crud_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (db, workspace_id) = seeded_sqlite_db().await;

    let input = CreateEntityInput {
        schema_name: "notes".into(),
        entity_type: "note".into(),
        data: serde_json::json!({"title": "first"}),
    };
    let created = content_entities::create(&db, workspace_id, input, None)
        .await
        .expect("create");
    assert_eq!(created.data["title"], "first");

    let fetched = content_entities::get(&db, workspace_id, created.id)
        .await
        .expect("get");
    assert_eq!(fetched.id, created.id);

    let batch = content_entities::get_batch(&db, workspace_id, &[created.id])
        .await
        .expect("get_batch");
    assert_eq!(batch.len(), 1);
    assert!(batch.contains_key(&created.id));

    let listed = content_entities::list(&db, workspace_id, ListEntitiesQuery::default())
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);

    let exported = content_entities::export_all(&db, workspace_id)
        .await
        .expect("export_all");
    assert_eq!(exported.len(), 1);

    let counted = content_entities::count(&db, workspace_id)
        .await
        .expect("count");
    assert_eq!(counted, 1);

    let updated = content_entities::update(
        &db,
        workspace_id,
        created.id,
        serde_json::json!({"title": "second"}),
        None,
    )
    .await
    .expect("update");
    assert_eq!(updated.data["title"], "second");
    assert!(
        updated.updated_at > created.updated_at,
        "updated_at should advance on update: created {:?}, updated {:?}",
        created.updated_at,
        updated.updated_at
    );

    content_entities::delete(&db, workspace_id, created.id)
        .await
        .expect("delete");

    let after_delete = content_entities::count(&db, workspace_id)
        .await
        .expect("count after delete");
    assert_eq!(after_delete, 0);
}

/// `undo_job` calls `ActiveModel::update(conn)` directly rather than going through `content_entities::update`, so it carries its own SQLite branch instead of inheriting `update_and_fetch`'s.
/// Guards both outcomes its `match` distinguishes: a snapshot whose entity still exists (`restored`) and one whose entity was deleted since (`missing`, via `DbErr::RecordNotUpdated`).
#[tokio::test]
async fn undo_job_restores_and_counts_a_missing_entity_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (db, workspace_id) = seeded_sqlite_db().await;

    let input = CreateEntityInput {
        schema_name: "notes".into(),
        entity_type: "note".into(),
        data: serde_json::json!({"title": "original"}),
    };
    let created = content_entities::create(&db, workspace_id, input, None)
        .await
        .expect("create");

    let schema_id = content_entities::get(&db, workspace_id, created.id)
        .await
        .expect("get")
        .schema_id;
    let job_id = uuid::Uuid::now_v7();

    // A snapshot for the entity that still exists: `undo_job` should restore it.
    let existing_snapshot_id = uuid::Uuid::now_v7();
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO content_entity_snapshots \
            (id, job_id, workspace_id, entity_id, schema_id, schema_version, data) \
         VALUES ($1, $2, $3, $4, $5, 1, $6)",
        [
            existing_snapshot_id.into(),
            job_id.into(),
            workspace_id.into(),
            created.id.into(),
            schema_id.into(),
            serde_json::json!({"title": "restored"}).to_string().into(),
        ],
    ))
    .await
    .expect("insert snapshot for the existing entity");

    // A snapshot for an entity that no longer exists: `undo_job` should count it as missing,
    // not fail the whole batch.
    let deleted_entity_id = uuid::Uuid::now_v7();
    let missing_snapshot_id = uuid::Uuid::now_v7();
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO content_entity_snapshots \
            (id, job_id, workspace_id, entity_id, schema_id, schema_version, data) \
         VALUES ($1, $2, $3, $4, $5, 1, $6)",
        [
            missing_snapshot_id.into(),
            job_id.into(),
            workspace_id.into(),
            deleted_entity_id.into(),
            schema_id.into(),
            serde_json::json!({"title": "gone"}).to_string().into(),
        ],
    ))
    .await
    .expect("insert snapshot for the deleted entity");

    let report = content_entities::undo_job(&db, workspace_id, job_id)
        .await
        .expect("undo_job");
    assert_eq!(report.restored, 1);
    assert_eq!(report.missing, 1);

    let restored = content_entities::get(&db, workspace_id, created.id)
        .await
        .expect("get after undo");
    assert_eq!(restored.data["title"], "restored");
}
