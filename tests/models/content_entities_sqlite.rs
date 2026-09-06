use migration::{Migrator, MigratorTrait};
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, Database};
use yorishiro::models::content_entities::{self, CreateEntityInput, ListEntitiesQuery};
use yorishiro::models::content_schemas;

async fn seeded_sqlite_db() -> (sea_orm::DatabaseConnection, uuid::Uuid) {
    yorishiro::db::register_sqlite_extensions();
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();

    let tenant = yorishiro::models::_entities::identity_tenants::ActiveModel {
        name: ActiveValue::Set("acme".into()),
        ..Default::default()
    };
    let tenant = tenant.insert(&db).await.expect("insert tenant");

    let workspace = yorishiro::models::_entities::identity_workspaces::ActiveModel {
        tenant_id: ActiveValue::Set(tenant.id),
        name: ActiveValue::Set("ws".into()),
        status: ActiveValue::Set("active".into()),
        ..Default::default()
    };
    let workspace = workspace.insert(&db).await.expect("insert workspace");

    let definition = serde_json::json!({
        "name": "notes",
        "entity_types": {
            "note": { "fields": {}, "required": [] }
        }
    });
    let def = serde_json::from_value(definition).expect("parse definition");
    content_schemas::create_schema(&db, tenant.id, workspace.id, def, None, None)
        .await
        .expect("create schema");

    (db, workspace.id)
}

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
        "updated_at should advance on update"
    );

    content_entities::delete(&db, workspace_id, created.id)
        .await
        .expect("delete");

    let after_delete = content_entities::count(&db, workspace_id)
        .await
        .expect("count after delete");
    assert_eq!(after_delete, 0);
}

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

    let entity = content_entities::get(&db, workspace_id, created.id)
        .await
        .expect("get");
    let job_id = uuid::Uuid::now_v7();

    let existing_snapshot = yorishiro::models::_entities::content_entity_snapshots::ActiveModel {
        job_id: ActiveValue::Set(job_id),
        workspace_id: ActiveValue::Set(workspace_id),
        entity_id: ActiveValue::Set(created.id),
        schema_id: ActiveValue::Set(entity.schema_id),
        schema_version: ActiveValue::Set(1),
        data: ActiveValue::Set(serde_json::json!({"title": "restored"})),
        ..Default::default()
    };
    existing_snapshot
        .insert(&db)
        .await
        .expect("insert snapshot for existing entity");

    let deleted_entity_id = uuid::Uuid::now_v7();
    let missing_snapshot = yorishiro::models::_entities::content_entity_snapshots::ActiveModel {
        job_id: ActiveValue::Set(job_id),
        workspace_id: ActiveValue::Set(workspace_id),
        entity_id: ActiveValue::Set(deleted_entity_id),
        schema_id: ActiveValue::Set(entity.schema_id),
        schema_version: ActiveValue::Set(1),
        data: ActiveValue::Set(serde_json::json!({"title": "gone"})),
        ..Default::default()
    };
    missing_snapshot
        .insert(&db)
        .await
        .expect("insert snapshot for deleted entity");

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
