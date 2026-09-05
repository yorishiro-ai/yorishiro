use migration::{Migrator, MigratorTrait};
/// SQLite-specific tests for content_entities: CRUD operations and snapshot undo.
use sea_orm::{ConnectionTrait, Database, Statement};
use yorishiro::models::content_entities::{self, CreateEntityInput, ListEntitiesQuery};

/// Smoke test: a minimal content_entities INSERT must succeed against the migrated schema.
/// Also exercises FTS5 trigger correctness (no UUID-as-rowid mismatch).
#[tokio::test]
async fn debug_content_entities_datatype_mismatch() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    // Enable FK enforcement (off by default in SQLite).
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();

    let tenant_id = uuid::Uuid::now_v7();
    let workspace_id = uuid::Uuid::now_v7();
    let schema_id = uuid::Uuid::now_v7();
    // TEXT columns need hex-string UUIDs; `Value::Uuid` serialises as binary which
    // cannot match the FK target when it lives in a TEXT column.
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO identity_tenants (id, name) VALUES (?1, 'debug')",
        [sea_orm::Value::String(Some(tenant_id.to_string()))],
    ))
    .await
    .expect("insert tenant");
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO identity_workspaces (id, tenant_id, name, status, max_entities) \
         VALUES (?1, ?2, 'ws', 'active', NULL)",
        [
            sea_orm::Value::String(Some(workspace_id.to_string())),
            sea_orm::Value::String(Some(tenant_id.to_string())),
        ],
    ))
    .await
    .expect("insert workspace");
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO content_schemas \
            (id, tenant_id, workspace_id, name, version, definition, status) \
         VALUES (?1, ?2, ?3, 'notes', 1, ?4, 'active')",
        [
            sea_orm::Value::String(Some(schema_id.to_string())),
            sea_orm::Value::String(Some(tenant_id.to_string())),
            sea_orm::Value::String(Some(workspace_id.to_string())),
            sea_orm::Value::String(Some(
                serde_json::json!({"name":"notes","entity_types":{"note":{}}}).to_string(),
            )),
        ],
    ))
    .await
    .expect("insert schema");

    // Full INSERT with all NOT NULL columns — exercises FTS5 trigger.
    let eid = uuid::Uuid::now_v7();
    let sql = format!(
        "INSERT INTO content_entities (id,workspace_id,schema_id,schema_version,entity_type,data) \
         VALUES ('{}','{}','{}',1,'note','{{\"x\":1}}')",
        eid, workspace_id, schema_id
    );
    db.execute_unprepared(&sql)
        .await
        .expect("INSERT content_entities");

    // Verify the row is visible
    let verify = format!(
        "SELECT entity_type,data FROM content_entities WHERE id = '{}'",
        eid
    );
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            &verify,
            vec![],
        ))
        .await
        .expect("SELECT content_entities");
    assert_eq!(rows.len(), 1, "row should be visible after INSERT");
}

/// A fresh in-memory SQLite database, migrated, with one tenant/workspace/schema seeded via raw SQL (not through `tenancy`/`content_schemas`, to keep this test focused on `content_entities` itself).
/// Mirrors `tenancy.rs`'s own `sqlite_db()` test helper.
async fn seeded_sqlite_db() -> (sea_orm::DatabaseConnection, uuid::Uuid) {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    // Enable FK enforcement (off by default in SQLite).
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();

    let tenant_id = uuid::Uuid::now_v7();
    let workspace_id = uuid::Uuid::now_v7();
    let schema_id = uuid::Uuid::now_v7();

    // TEXT columns need hex-string UUIDs; `Value::Uuid` serialises as binary which
    // cannot match the FK target when it lives in a TEXT column.
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO identity_tenants (id, name) VALUES (?1, 'acme')",
        [sea_orm::Value::String(Some(tenant_id.to_string()))],
    ))
    .await
    .expect("insert tenant");

    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "INSERT INTO identity_workspaces (id, tenant_id, name, status, max_entities) \
         VALUES (?1, ?2, 'ws', 'active', NULL)",
        [
            sea_orm::Value::String(Some(workspace_id.to_string())),
            sea_orm::Value::String(Some(tenant_id.to_string())),
        ],
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
         VALUES (?1, ?2, ?3, 'notes', 1, ?4, 'active')",
        [
            sea_orm::Value::String(Some(schema_id.to_string())),
            sea_orm::Value::String(Some(tenant_id.to_string())),
            sea_orm::Value::String(Some(workspace_id.to_string())),
            sea_orm::Value::String(Some(definition.to_string())),
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

    // Debug: verify schema is visible to SeaORM
    use yorishiro::models::content_schemas;
    let schema = content_schemas::get_active_schema(&db, workspace_id, "notes").await;
    println!("  Debug: get_active_schema = {:?}", schema);

    // Debug: try SeaORM find_all without filters
    use sea_orm::EntityTrait;
    let all_schemas = yorishiro::models::_entities::content_schemas::Entity::find()
        .all(&db)
        .await;
    println!(
        "  Debug: all schemas count = {}",
        all_schemas.as_ref().map(|v| v.len()).unwrap_or(0)
    );
    for s in all_schemas.unwrap_or_default() {
        println!(
            "  Debug: schema name={}, workspace_id={}",
            s.name, s.workspace_id
        );
    }

    // Debug: raw query to see what's in the table
    let raw_rows = db
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT id, workspace_id, name, version, status FROM content_schemas",
            vec![],
        ))
        .await;
    match raw_rows {
        Ok(rows) => {
            for row in &rows {
                let id: String = row.try_get("", "id").unwrap_or_default();
                let name: String = row.try_get("", "name").unwrap_or_default();
                let status: String = row.try_get("", "status").unwrap_or_default();
                println!("  Debug: raw row: id={id}, name={name}, status={status}");
            }
        }
        Err(e) => println!("  Debug: raw SELECT error: {e}"),
    }

    // Debug: raw count with String placeholder (what SeaORM should be generating)
    let hex_wid = workspace_id.to_string();
    let raw_count = db.query_all_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "SELECT count(*) FROM content_schemas WHERE workspace_id = ? AND name = 'notes' AND status = 'active'",
        [sea_orm::Value::String(Some(hex_wid))],
    )).await;
    println!("  Debug: raw count with String = {raw_count:?}");

    // Debug: raw count with binary UUID (what SeaORM might actually generate)
    use sea_orm::Value;
    let binary_val: Value = workspace_id.into();
    let raw_count_binary = db.query_all_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "SELECT count(*) FROM content_schemas WHERE workspace_id = ? AND name = 'notes' AND status = 'active'",
        [binary_val],
    )).await;
    println!("  Debug: raw count with binary Uuid = {raw_count_binary:?}");

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
    db.execute_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Sqlite,
        format!(
            "INSERT INTO content_entity_snapshots \
                (id, job_id, workspace_id, entity_id, schema_id, schema_version, data) \
             VALUES ('{}', '{}', '{}', '{}', '{}', 1, '{}')",
            existing_snapshot_id,
            job_id,
            workspace_id,
            created.id,
            schema_id,
            serde_json::json!({"title": "restored"})
                .to_string()
                .replace('\'', "''")
        ),
    ))
    .await
    .expect("insert snapshot for the existing entity");

    // A snapshot for an entity that no longer exists: `undo_job` should count it as missing,
    // not fail the whole batch.
    let deleted_entity_id = uuid::Uuid::now_v7();
    let missing_snapshot_id = uuid::Uuid::now_v7();
    db.execute_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Sqlite,
        format!(
            "INSERT INTO content_entity_snapshots \
                (id, job_id, workspace_id, entity_id, schema_id, schema_version, data) \
             VALUES ('{}', '{}', '{}', '{}', '{}', 1, '{}')",
            missing_snapshot_id,
            job_id,
            workspace_id,
            deleted_entity_id,
            schema_id,
            serde_json::json!({"title": "gone"})
                .to_string()
                .replace('\'', "''")
        ),
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
