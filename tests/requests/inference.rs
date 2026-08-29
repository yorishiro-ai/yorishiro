use chrono::Utc;
use loco_rs::testing::prelude::*;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Statement};
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::db::DbHandle;
use yorishiro::ee::models::entity_fill;
use yorishiro::ee::services::licence::{LicenceClaims, LicenceState};
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

/// `shared_store.insert` is keyed by `TypeId`, so this overwrites the `LicenceState::from_env()` the test process booted with.
/// See `marketplace.rs`'s own copy of this helper.
fn licence(ctx: &loco_rs::app::AppContext) {
    ctx.shared_store
        .insert(LicenceState::licensed(LicenceClaims {
            sub: "acme-corp".into(),
            plan: "enterprise".into(),
            exp: Utc::now().timestamp() + 60 * 60,
        }));
}

struct Setup {
    key: String,
    tenant_id: Uuid,
    workspace_id: Uuid,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    // Migration, not Schema: ApiKeyScope::Schema (infer_fill's own requirement) is a lower rung
    // than Migration (POST /api/migration-jobs/{job_id}/undo's requirement, see
    // controllers::entities::undo_migration_job), and Migration subsumes it, so one key issued at
    // the higher scope satisfies both this file's infer_fill calls and its undo call.
    let key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Migration,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup {
        key,
        tenant_id: tenant.id,
        workspace_id: workspace.id,
    }
}

/// Setting, reading and clearing a workspace's LLM credentials over REST.
/// The key itself never comes back from GET, only what it configured.
#[tokio::test]
#[serial]
async fn llm_key_set_get_and_clear_round_trip() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let missing = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(missing.status_code(), 404, "response: {:?}", missing.text());

        let put = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1/",
                "model": "gpt-4o-mini",
                "api_key": "sk-secret-value"
            }))
            .await;
        assert_eq!(put.status_code(), 204, "response: {:?}", put.text());

        let get = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(get.status_code(), 200, "response: {:?}", get.text());
        let body: serde_json::Value = get.json();
        // The trailing slash is trimmed once at write time.
        assert_eq!(body["base_url"], "https://api.example.com/v1");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["configured"], true);
        let rendered = get.text();
        assert!(
            !rendered.contains("sk-secret-value"),
            "the key must never be returned: {rendered}"
        );

        let delete = request
            .delete("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(delete.status_code(), 204, "response: {:?}", delete.text());

        let after_delete = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(after_delete.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A scheme that could never be a chat-completions endpoint is refused before anything is stored, and a URL with no scheme at all is refused too rather than becoming a relative path.
#[tokio::test]
#[serial]
async fn a_non_http_base_url_is_refused() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        for bad_url in [
            "file:///etc/passwd",
            "gopher://example.com",
            "api.example.com/v1",
        ] {
            let put = request
                .put("/api/workspace/llm-key")
                .add_header("Authorization", format!("Bearer {}", setup.key))
                .json(&serde_json::json!({
                    "base_url": bad_url,
                    "model": "m",
                    "api_key": "k"
                }))
                .await;
            assert_eq!(
                put.status_code(),
                422,
                "{bad_url:?} should have been refused: {:?}",
                put.text()
            );
        }

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A workspace with no credentials configured is refused with one clear error before any entity is scanned, rather than reporting zero applied in a way that reads as "nothing to infer".
#[tokio::test]
#[serial]
async fn infer_fill_without_a_configured_key_is_refused() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;

        let create_schema = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "name": "note",
                "entity_types": {
                    "note": { "fields": { "title": { "type": "string", "required": true } } }
                }
            }))
            .await;
        assert_eq!(
            create_schema.status_code(),
            201,
            "response: {:?}",
            create_schema.text()
        );

        let infer = request
            .post("/api/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(infer.status_code(), 422, "response: {:?}", infer.text());

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// An unlicensed deployment answers the same 404 whether or not a valid key is presented, so an anonymous prober cannot tell "does not exist" from "exists but locked".
/// Matches `marketplace`'s and `dashboard`'s own tests for the same gate.
#[tokio::test]
#[serial]
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let with_key = request
            .post("/api/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let without_key = request.post("/api/schemas/active/note/infer-fill").await;

        assert_eq!(with_key.status_code(), without_key.status_code());
        assert_eq!(with_key.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// Creates one schema, one entity on it, and returns the entity's id alongside a
/// `entity_fill::OutdatedEntity` view of it (as `entities_on_outdated_schema` would produce),
/// for tests exercising `apply_answers` directly without a real or stubbed LLM endpoint.
async fn create_entity(
    request: &axum_test::TestServer,
    setup: &Setup,
) -> entity_fill::OutdatedEntity {
    let create_schema = request
        .post("/api/schemas")
        .add_header("Authorization", format!("Bearer {}", setup.key))
        .json(&serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "summary": { "type": "string" }
                    }
                }
            }
        }))
        .await;
    assert_eq!(
        create_schema.status_code(),
        201,
        "response: {:?}",
        create_schema.text()
    );

    let create_entity = request
        .post("/api/entities")
        .add_header("Authorization", format!("Bearer {}", setup.key))
        .json(&serde_json::json!({
            "schema_name": "note",
            "entity_type": "note",
            "data": { "title": "original" }
        }))
        .await;
    assert_eq!(
        create_entity.status_code(),
        201,
        "response: {:?}",
        create_entity.text()
    );
    let entity_id: Uuid = create_entity.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    entity_fill::OutdatedEntity {
        id: entity_id,
        entity_type: "note".to_string(),
        data: serde_json::json!({ "title": "original" }),
    }
}

/// `apply_answers` writes a model's already-resolved answer straight into `content_entities`, with
/// no separate confirm step, and the snapshot it takes is readable by base's own, unchanged
/// `POST /api/migration-jobs/{job_id}/undo`: this is `infer_fill`'s own write path, factored out
/// so it is testable without a real or stubbed LLM endpoint.
#[tokio::test]
#[serial]
async fn apply_answers_writes_directly_and_undo_reverses_it() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;
        let entity = create_entity(&request, &setup).await;

        let db = ctx.shared_store.get::<DbHandle>().unwrap();
        let job_id = Uuid::new_v4();
        {
            let txn = db
                .tenant
                .begin_for_workspace(setup.tenant_id, setup.workspace_id)
                .await
                .expect("begin tenant txn");
            let mut answers = serde_json::Map::new();
            answers.insert("summary".to_string(), serde_json::json!("a stub summary"));
            let applied =
                entity_fill::apply_answers(&txn, setup.workspace_id, &entity, job_id, answers)
                    .await
                    .expect("apply_answers");
            assert!(applied, "the write should have landed");
            txn.commit().await.expect("commit apply");
        }

        let get_entity = request
            .get(&format!("/api/entities/{}", entity.id))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            get_entity.status_code(),
            200,
            "response: {:?}",
            get_entity.text()
        );
        let fetched: serde_json::Value = get_entity.json();
        assert_eq!(
            fetched["data"]["summary"], "a stub summary",
            "the answer must be written directly, not merely recorded: {fetched:?}"
        );

        // POST /api/migration-jobs/{job_id}/undo is base's own, unchanged endpoint: apply_answers's
        // snapshot must be readable by it with no glue code of its own.
        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(undo.status_code(), 200, "response: {:?}", undo.text());
        let undo_report: serde_json::Value = undo.json();
        assert_eq!(undo_report["restored"], 1, "undo report: {undo_report:?}");

        let get_after_undo = request
            .get(&format!("/api/entities/{}", entity.id))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let fetched_after_undo: serde_json::Value = get_after_undo.json();
        assert!(
            fetched_after_undo["data"].get("summary").is_none(),
            "undo must restore the pre-inference state: {fetched_after_undo:?}"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A write that fails for a reason specific to this entity (its schema no longer accepts the
/// merged data) must not leave a snapshot behind: leaving one would let a later, unrelated edit to
/// the same entity be misattributed to this job on undo.
#[tokio::test]
#[serial]
async fn apply_answers_removes_its_snapshot_when_the_write_is_rejected() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;
        let entity = create_entity(&request, &setup).await;

        let db = ctx.shared_store.get::<DbHandle>().unwrap();
        let job_id = Uuid::new_v4();
        let txn = db
            .tenant
            .begin_for_workspace(setup.tenant_id, setup.workspace_id)
            .await
            .expect("begin tenant txn");
        // "summary" has no declared type constraint that would reject a value, so a non-string answer for a field the schema does declare as a string is what actually gets refused: content_entities::update validates the merged data against the schema, and this shape does not match it.
        let mut answers = serde_json::Map::new();
        answers.insert("title".to_string(), serde_json::json!(12345));
        let applied =
            entity_fill::apply_answers(&txn, setup.workspace_id, &entity, job_id, answers)
                .await
                .expect("apply_answers");
        assert!(
            !applied,
            "a schema-rejected write must be reported as skipped"
        );

        // No snapshot should remain for this job_id.
        let remaining = yorishiro::models::_entities::content_entity_snapshots::Entity::find()
            .filter(
                yorishiro::models::_entities::content_entity_snapshots::Column::WorkspaceId
                    .eq(setup.workspace_id),
            )
            .filter(
                yorishiro::models::_entities::content_entity_snapshots::Column::JobId.eq(job_id),
            )
            .count(&txn)
            .await
            .expect("count snapshots");
        assert_eq!(remaining, 0, "a rejected write must not leave a snapshot");

        txn.rollback().await.expect("rollback txn");
        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// Sets up one schema and one entity, then locks `content_entities` in `lock_mode` from a second
/// connection and calls `apply_answers` with a short `lock_timeout`, returning its result as a
/// string (its `Err` message, or `"Ok(bool)"` if it somehow succeeded) for the caller to assert on.
///
/// `yorishiro_app` is granted per-table (see loco-architecture.md), not the owner of any table, so it cannot DROP or REVOKE its own way into a failure; locking the table from a second connection is a failure `apply_answers` can genuinely hit without needing privileges the RLS role does not have.
async fn apply_answers_with_content_entities_locked(
    request: &axum_test::TestServer,
    ctx: &loco_rs::app::AppContext,
    lock_mode: &'static str,
) -> String {
    let setup = setup(ctx).await;
    let entity = create_entity(request, &setup).await;

    let db = ctx.shared_store.get::<DbHandle>().unwrap();
    let mut blocker = db.identity.acquire().await.expect("acquire blocker conn");
    sqlx::query("BEGIN")
        .execute(&mut *blocker)
        .await
        .expect("begin blocker txn");
    let lock_statement = match lock_mode {
        "EXCLUSIVE" => "LOCK TABLE content_entities IN EXCLUSIVE MODE",
        "ACCESS EXCLUSIVE" => "LOCK TABLE content_entities IN ACCESS EXCLUSIVE MODE",
        other => panic!("unexpected lock mode {other:?}"),
    };
    sqlx::query(lock_statement)
        .execute(&mut *blocker)
        .await
        .expect("lock content_entities");

    let job_id = Uuid::new_v4();
    let result = {
        let txn = db
            .tenant
            .begin_for_workspace(setup.tenant_id, setup.workspace_id)
            .await
            .expect("begin tenant txn");
        txn.execute_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SET LOCAL lock_timeout = '50ms'",
        ))
        .await
        .expect("set lock_timeout");

        let mut answers = serde_json::Map::new();
        answers.insert("summary".to_string(), serde_json::json!("a stub summary"));
        let result =
            entity_fill::apply_answers(&txn, setup.workspace_id, &entity, job_id, answers).await;
        // Rolls back on drop: the lock_timeout is transaction-local, and anything apply_answers
        // did before failing is undone along with it.
        match result {
            Ok(applied) => format!("Ok({applied})"),
            Err(err) => err.to_string(),
        }
    };

    sqlx::query("ROLLBACK")
        .execute(&mut *blocker)
        .await
        .expect("release blocker lock");
    // The pool does not consider this connection returned until it drops: the next
    // begin_for_workspace call hangs until this one is, so it must go before that call.
    drop(blocker);

    result
}

/// `EXCLUSIVE` still admits the plain `SELECT` `content_entities::get` runs inside `update`, so the
/// failure lands specifically on `update`'s write, exercising `apply_answers`'s
/// `Err(err) => Err(err)` arm rather than the one on the snapshot's own read.
#[tokio::test]
#[serial]
async fn an_infrastructure_failure_surfaces_as_itself_not_a_masked_abort_error() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let message = apply_answers_with_content_entities_locked(&request, &ctx, "EXCLUSIVE").await;
        assert!(
            message.contains("lock timeout"),
            "expected the lock timeout itself, not a downstream error: {message}"
        );
        assert!(
            !message.contains("transaction is aborted"),
            "the old code's masked failure mode must not reappear: {message}"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// `ACCESS EXCLUSIVE` also blocks `snapshot`'s own `INSERT ... SELECT`, isolating the failure to
/// that statement instead of `update`'s.
#[tokio::test]
#[serial]
async fn an_infrastructure_failure_on_snapshot_surfaces_as_itself() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let message =
            apply_answers_with_content_entities_locked(&request, &ctx, "ACCESS EXCLUSIVE").await;
        assert!(
            message.contains("lock timeout"),
            "expected the lock timeout itself, not a downstream error: {message}"
        );
        assert!(
            !message.contains("transaction is aborted"),
            "the old code's masked failure mode must not reappear: {message}"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}
