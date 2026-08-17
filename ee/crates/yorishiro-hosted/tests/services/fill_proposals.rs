use super::*;
use crate::tests::test_helpers::{seed_tenant, seed_workspace};
use sqlx::PgPool;
use yorishiro_core::repositories::{entities, schemas};

/// A schema with one required field and one the model might fill in.
async fn seed_entity(pool: &PgPool) -> (Uuid, Uuid) {
    let tenant_id = seed_tenant(pool, "t").await;
    let workspace_id = seed_workspace(pool, tenant_id, "w").await;
    let _ = &tenant_id;

    let mut conn = pool.acquire().await.unwrap();
    let definition = serde_json::from_value(serde_json::json!({
        "name": "notes",
        "entity_types": {
            "note": { "fields": {
                "title": { "type": "string", "required": true },
                "category": { "type": "string" }
            } }
        }
    }))
    .unwrap();
    schemas::create_schema(&mut conn, tenant_id, workspace_id, definition)
        .await
        .unwrap();

    let record = entities::create(
        &mut conn,
        workspace_id,
        entities::CreateEntityInput {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: serde_json::json!({ "title": "a note" }),
        },
        None,
    )
    .await
    .unwrap();

    (workspace_id, record.id)
}

/// A proposal is not a write. Recording one must leave the entity exactly as it was: the
/// whole reason mode B holds its output here is that a guess written straight into an entity
/// becomes indistinguishable from a value someone entered.
#[sqlx::test(migrations = "../../../migrations")]
async fn recording_a_proposal_does_not_touch_the_entity(pool: PgPool) {
    let (workspace_id, entity_id) = seed_entity(&pool).await;
    let job_id = uuid::Uuid::nil();
    let mut conn = pool.acquire().await.unwrap();

    record(
        &mut conn,
        workspace_id,
        job_id,
        entity_id,
        "category",
        &serde_json::json!("fiction"),
    )
    .await
    .unwrap();

    let entity = entities::get(&mut conn, workspace_id, entity_id)
        .await
        .unwrap();
    assert!(
        entity.data.get("category").is_none(),
        "the entity must be untouched until the job is confirmed"
    );

    let proposals = for_job(&mut conn, workspace_id, job_id).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].proposed, serde_json::json!("fiction"));
}

/// Confirming writes the reviewed values, and `undo_job` reverses the whole thing: the point
/// of reusing the snapshot machinery rather than adding a second rollback path.
#[sqlx::test(migrations = "../../../migrations")]
async fn confirming_applies_the_proposals_and_undo_reverses_them(pool: PgPool) {
    let (workspace_id, entity_id) = seed_entity(&pool).await;
    let job_id = uuid::Uuid::nil();
    let mut conn = pool.acquire().await.unwrap();

    record(
        &mut conn,
        workspace_id,
        job_id,
        entity_id,
        "category",
        &serde_json::json!("fiction"),
    )
    .await
    .unwrap();

    let report = confirm(&mut conn, workspace_id, job_id).await.unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.skipped, 0);

    let after = entities::get(&mut conn, workspace_id, entity_id)
        .await
        .unwrap();
    assert_eq!(after.data["category"], serde_json::json!("fiction"));

    entities::undo_job(&mut conn, workspace_id, job_id)
        .await
        .unwrap();

    let restored = entities::get(&mut conn, workspace_id, entity_id)
        .await
        .unwrap();
    assert!(
        restored.data.get("category").is_none(),
        "undo must restore the state that was reviewed away from"
    );
}

/// Confirming clears the job. Leaving the proposals would let the same job be confirmed again
/// after an undo, writing the same guesses back over what the undo restored.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_job_cannot_be_confirmed_twice(pool: PgPool) {
    let (workspace_id, entity_id) = seed_entity(&pool).await;
    let job_id = uuid::Uuid::nil();
    let mut conn = pool.acquire().await.unwrap();

    record(
        &mut conn,
        workspace_id,
        job_id,
        entity_id,
        "category",
        &serde_json::json!("fiction"),
    )
    .await
    .unwrap();
    confirm(&mut conn, workspace_id, job_id).await.unwrap();

    assert!(
        confirm(&mut conn, workspace_id, job_id).await.is_err(),
        "the second confirmation has nothing to apply and must say so"
    );
}

/// A guess the schema rejects is skipped, not fatal. The rest of a batch someone reviewed
/// should still land: discarding all of it because one field read badly would push a reviewer
/// toward accepting everything.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_proposal_the_schema_rejects_is_skipped(pool: PgPool) {
    let (workspace_id, entity_id) = seed_entity(&pool).await;
    let job_id = uuid::Uuid::nil();
    let mut conn = pool.acquire().await.unwrap();

    // `title` is a string; a number must not be written.
    record(
        &mut conn,
        workspace_id,
        job_id,
        entity_id,
        "title",
        &serde_json::json!(42),
    )
    .await
    .unwrap();

    let report = confirm(&mut conn, workspace_id, job_id).await.unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.skipped, 1);

    let after = entities::get(&mut conn, workspace_id, entity_id)
        .await
        .unwrap();
    assert_eq!(after.data["title"], serde_json::json!("a note"));
}

/// Re-running inference for a job replaces the earlier answer rather than adding a second one,
/// which would leave the choice between them to row order.
#[sqlx::test(migrations = "../../../migrations")]
async fn recording_the_same_field_twice_replaces_the_proposal(pool: PgPool) {
    let (workspace_id, entity_id) = seed_entity(&pool).await;
    let job_id = uuid::Uuid::nil();
    let mut conn = pool.acquire().await.unwrap();

    for value in ["first", "second"] {
        record(
            &mut conn,
            workspace_id,
            job_id,
            entity_id,
            "category",
            &serde_json::json!(value),
        )
        .await
        .unwrap();
    }

    let proposals = for_job(&mut conn, workspace_id, job_id).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].proposed, serde_json::json!("second"));
}
