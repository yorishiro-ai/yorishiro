use migration::{Migrator, MigratorTrait};
use sea_orm::{ActiveModelTrait, ActiveValue, Database};
use serde_json::json;
use uuid::Uuid;
use yorishiro::ee::models::{inference_jobs, inference_proposals};
use yorishiro::models::{entity_entities, schema_schemas};

async fn database() -> (
    sea_orm::DatabaseConnection,
    Uuid,
    entity_entities::EntityRecord,
) {
    yorishiro::db::register_sqlite_extensions();
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect database");
    Migrator::up(&db, None).await.expect("migrate database");

    let tenant = yorishiro::models::_entities::tenant_tenants::ActiveModel {
        name: ActiveValue::Set("proposal-tenant".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert tenant");
    let workspace = yorishiro::models::_entities::workspace_workspaces::ActiveModel {
        tenant_id: ActiveValue::Set(tenant.id),
        name: ActiveValue::Set("proposal-workspace".into()),
        status: ActiveValue::Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert workspace");

    let v1 = serde_json::from_value(json!({
        "name": "notes",
        "entity_types": {"note": {"fields": {"title": {"type": "string", "required": true}}}}
    }))
    .expect("parse v1");
    schema_schemas::create_schema(&db, tenant.id, workspace.id, v1, None, None)
        .await
        .expect("create v1");
    let entity = entity_entities::create(
        &db,
        workspace.id,
        entity_entities::CreateEntityInput {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: json!({"title": "original"}),
        },
        None,
    )
    .await
    .expect("create entity");

    let v2 = serde_json::from_value(json!({
        "name": "notes",
        "entity_types": {"note": {"fields": {
            "title": {"type": "string", "required": true},
            "body": {"type": "string", "required": true}
        }}}
    }))
    .expect("parse v2");
    schema_schemas::create_schema(&db, tenant.id, workspace.id, v2, None, None)
        .await
        .expect("create v2");

    (db, workspace.id, entity)
}

#[tokio::test]
async fn proposals_are_idempotent_and_confirmation_is_validated_and_undoable() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (db, workspace_id, entity) = database().await;
    let active = schema_schemas::get_active_schema(&db, workspace_id, "notes")
        .await
        .expect("active schema");
    let job_id = Uuid::now_v7();
    inference_jobs::create(&db, job_id, workspace_id, "notes")
        .await
        .expect("create job");
    inference_jobs::claim(&db, job_id).await.expect("claim job");
    let first = inference_proposals::record_batch(
        &db,
        workspace_id,
        job_id,
        active.id,
        active.version,
        [(entity.id, "body".into(), json!("inferred"))],
    )
    .await
    .expect("record proposal");
    let retry = inference_proposals::record_batch(
        &db,
        workspace_id,
        job_id,
        active.id,
        active.version,
        [(entity.id, "body".into(), json!("inferred"))],
    )
    .await
    .expect("retry proposal");
    assert_eq!(first, 1);
    assert_eq!(retry, 0);
    inference_jobs::complete(&db, job_id, 0, 0)
        .await
        .expect("complete job");
    assert_eq!(
        entity_entities::get(&db, workspace_id, entity.id)
            .await
            .unwrap()
            .data,
        json!({"title": "original"})
    );

    let txn = sea_orm::TransactionTrait::begin(&db).await.unwrap();
    let report = inference_proposals::confirm(&txn, workspace_id, job_id, None)
        .await
        .expect("confirm proposals");
    txn.commit().await.unwrap();
    assert_eq!(report.confirmed, 1);
    let confirmed = entity_entities::get(&db, workspace_id, entity.id)
        .await
        .unwrap();
    assert_eq!(confirmed.data["body"], "inferred");
    assert_eq!(confirmed.schema_id, active.id);

    assert!(
        inference_proposals::confirm(&db, workspace_id, job_id, None)
            .await
            .is_err()
    );
    let undo = entity_entities::undo_job(&db, workspace_id, job_id)
        .await
        .expect("undo confirmation");
    assert_eq!(undo.restored, 1);
    assert_eq!(
        entity_entities::get(&db, workspace_id, entity.id)
            .await
            .unwrap()
            .data,
        json!({"title": "original"})
    );
}

#[tokio::test]
async fn invalid_rejected_and_discarded_proposals_never_write_entities() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (db, workspace_id, entity) = database().await;
    let active = schema_schemas::get_active_schema(&db, workspace_id, "notes")
        .await
        .expect("active schema");

    let invalid_job = Uuid::now_v7();
    inference_jobs::create(&db, invalid_job, workspace_id, "notes")
        .await
        .unwrap();
    inference_jobs::claim(&db, invalid_job).await.unwrap();
    inference_proposals::record_batch(
        &db,
        workspace_id,
        invalid_job,
        active.id,
        active.version,
        [(entity.id, "body".into(), json!(42))],
    )
    .await
    .unwrap();
    inference_jobs::complete(&db, invalid_job, 0, 0)
        .await
        .unwrap();
    let invalid = inference_proposals::confirm(&db, workspace_id, invalid_job, None)
        .await
        .unwrap();
    assert_eq!(invalid.invalid, 1);

    for (job_id, rejected) in [(Uuid::now_v7(), true), (Uuid::now_v7(), false)] {
        inference_jobs::create(&db, job_id, workspace_id, "notes")
            .await
            .unwrap();
        inference_jobs::claim(&db, job_id).await.unwrap();
        inference_proposals::record_batch(
            &db,
            workspace_id,
            job_id,
            active.id,
            active.version,
            [(entity.id, "body".into(), json!("never applied"))],
        )
        .await
        .unwrap();
        inference_jobs::complete(&db, job_id, 0, 0).await.unwrap();
        let changed = if rejected {
            inference_proposals::reject(&db, workspace_id, job_id)
                .await
                .unwrap()
                .changed
        } else {
            inference_proposals::discard(&db, workspace_id, job_id)
                .await
                .unwrap()
                .changed
        };
        assert_eq!(changed, 1);
    }
    assert_eq!(
        entity_entities::get(&db, workspace_id, entity.id)
            .await
            .unwrap()
            .data,
        json!({"title": "original"})
    );
}

#[tokio::test]
async fn incomplete_and_failed_jobs_cannot_change_proposals() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (db, workspace_id, entity) = database().await;
    let active = schema_schemas::get_active_schema(&db, workspace_id, "notes")
        .await
        .unwrap();

    for failed in [false, true] {
        let job_id = Uuid::now_v7();
        inference_jobs::create(&db, job_id, workspace_id, "notes")
            .await
            .unwrap();
        inference_jobs::claim(&db, job_id).await.unwrap();
        inference_proposals::record_batch(
            &db,
            workspace_id,
            job_id,
            active.id,
            active.version,
            [(entity.id, "body".into(), json!("not actionable"))],
        )
        .await
        .unwrap();
        if failed {
            inference_jobs::fail(&db, job_id, "provider failed")
                .await
                .unwrap();
        }
        assert!(
            inference_proposals::confirm(&db, workspace_id, job_id, None)
                .await
                .is_err()
        );
        assert!(
            inference_proposals::reject(&db, workspace_id, job_id)
                .await
                .is_err()
        );
        assert!(
            inference_proposals::discard(&db, workspace_id, job_id)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn edited_entities_make_pending_proposals_stale() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let (db, workspace_id, entity) = database().await;
    let active = schema_schemas::get_active_schema(&db, workspace_id, "notes")
        .await
        .unwrap();
    let job_id = Uuid::now_v7();
    inference_jobs::create(&db, job_id, workspace_id, "notes")
        .await
        .unwrap();
    inference_jobs::claim(&db, job_id).await.unwrap();
    inference_proposals::record_batch(
        &db,
        workspace_id,
        job_id,
        active.id,
        active.version,
        [(entity.id, "body".into(), json!("inferred"))],
    )
    .await
    .unwrap();
    inference_jobs::complete(&db, job_id, 0, 0).await.unwrap();

    entity_entities::update(
        &db,
        workspace_id,
        entity_entities::UpdateEntityInput {
            id: entity.id,
            data: json!({"title": "edited", "body": "already filled"}),
            updated_by: None,
        },
    )
    .await
    .unwrap();
    let report = inference_proposals::confirm(&db, workspace_id, job_id, None)
        .await
        .unwrap();
    assert_eq!(report.stale, 1);
    assert_eq!(
        entity_entities::get(&db, workspace_id, entity.id)
            .await
            .unwrap()
            .data,
        json!({"title": "edited", "body": "already filled"})
    );
}
