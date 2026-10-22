use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use axum::http::StatusCode;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use std::sync::Arc;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::db::DbHandle;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
use yorishiro::edition::ee::models::inference_jobs::{self, InferenceJobStatus};

fn require_licence(ctx: &loco_rs::app::AppContext) {
    assert!(
        ctx.shared_store
            .get::<Arc<LicenceState>>()
            .expect("production boot installs licence state")
            .is_active(),
        "licensed inference tests require a valid YORISHIRO_LICENSE_KEY at boot"
    );
}

/// Setting, reading and clearing a workspace's LLM credentials over REST.
/// The key itself never comes back from GET, only what it configured.
#[tokio::test]
async fn llm_key_set_get_and_clear_round_trip() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let missing = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            missing.status_code(),
            StatusCode::NOT_FOUND,
            "response: {:?}",
            missing.text()
        );

        let put = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1/",
                "model": "gpt-4o-mini",
                "api_key": "sk-secret-value"
            }))
            .await;
        assert_eq!(
            put.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            put.text()
        );

        let get = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            get.status_code(),
            StatusCode::OK,
            "response: {:?}",
            get.text()
        );
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            delete.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            delete.text()
        );

        let after_delete = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(after_delete.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// A scheme that could never be a chat-completions endpoint is refused before anything is stored, and a URL with no scheme at all is refused too rather than becoming a relative path.
#[tokio::test]
async fn a_non_http_base_url_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        for bad_url in [
            "file:///etc/passwd",
            "gopher://example.com",
            "api.example.com/v1",
        ] {
            let put = request
                .put("/api/workspace/llm-key")
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
    })
    .await;
}

/// A workspace with no credentials configured is refused with one clear error before any entity is scanned, rather than reporting zero applied in a way that reads as "nothing to infer".
#[tokio::test]
async fn infer_fill_without_a_configured_key_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let create_schema = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            infer.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "response: {:?}",
            infer.text()
        );
    })
    .await;
}

/// An unlicensed deployment answers the same 404 whether or not a valid key is presented, so an anonymous prober cannot tell "does not exist" from "exists but locked".
/// Matches `marketplace`'s and `dashboard`'s own tests for the same gate.
#[tokio::test]
#[serial_test::serial(process_environment)]
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key() {
    let guard = crate::EnvGuard::capture(&["YORISHIRO_LICENSE_KEY"]);
    guard.remove("YORISHIRO_LICENSE_KEY");
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let with_key = request
            .post("/api/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        let without_key = request.post("/api/schemas/active/note/infer-fill").await;

        assert_eq!(with_key.status_code(), without_key.status_code());
        assert_eq!(with_key.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// Creates one schema and one entity for the infer-fill request tests.
pub(crate) async fn create_entity(request: &loco_rs::TestServer, setup: &TestTenant) -> Uuid {
    let create_schema = request
        .post("/api/schemas")
        .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
        .add_header("Authorization", format!("Bearer {}", setup.api_key))
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

    entity_id
}

/// A proposal remains separate from the entity until an explicit confirmation.
#[tokio::test]
async fn proposals_require_explicit_confirmation_and_undo_reverses_it() {
    super::fill_defaults::boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let entity = create_entity(&request, &setup).await;

        let job_id = Uuid::new_v4();
        inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
            .await
            .expect("create proposal job");
        inference_jobs::claim(&ctx.db, job_id)
            .await
            .expect("claim proposal job");
        {
            let txn = super::workspace_txn(&ctx, setup.tenant_id, setup.workspace_id).await;
            let schema = yorishiro::models::schema_schemas::get_active_schema(
                &txn,
                setup.workspace_id,
                "note",
            )
            .await
            .expect("active schema");
            yorishiro::edition::ee::models::inference_proposals::record_batch(
                &txn,
                setup.workspace_id,
                job_id,
                schema.id,
                schema.version,
                [(
                    entity,
                    "summary".into(),
                    serde_json::json!("a stub summary"),
                )],
            )
            .await
            .expect("record proposal");
            inference_jobs::complete(&txn, job_id, 0, 0)
                .await
                .expect("complete proposal job");
            txn.commit().await.expect("commit apply");
        }

        let get_entity = request
            .get(&format!("/api/entities/{}", entity))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            get_entity.status_code(),
            200,
            "response: {:?}",
            get_entity.text()
        );
        let fetched: serde_json::Value = get_entity.json();
        assert!(
            fetched["data"].get("summary").is_none(),
            "the proposal must not be written before confirmation: {fetched:?}"
        );

        let confirm = request
            .post(&format!("/api/inference-jobs/{job_id}/confirm"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            confirm.status_code(),
            StatusCode::ACCEPTED,
            "response: {:?}",
            confirm.text()
        );
        let accepted: serde_json::Value = confirm.json();
        let confirmation_id = accepted["job_id"].as_str().unwrap().parse().unwrap();
        super::fill_defaults::finish_via_api(
            &request,
            &ctx,
            confirmation_id,
            &setup.api_key,
            &format!("/api/inference-jobs/confirmations/{confirmation_id}"),
        )
        .await;
        let status = request
            .get(&format!(
                "/api/inference-jobs/confirmations/{confirmation_id}"
            ))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(status.status_code(), StatusCode::OK, "{}", status.text());
        assert_eq!(status.json::<serde_json::Value>()["result"]["confirmed"], 1);

        // The accepted write is snapshot-backed and remains undoable through the base endpoint.
        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            undo.status_code(),
            StatusCode::ACCEPTED,
            "response: {:?}",
            undo.text()
        );
        let undo_report: serde_json::Value = undo.json();
        let undo_id = undo_report["job_id"].as_str().unwrap().parse().unwrap();
        super::fill_defaults::finish_via_api(
            &request,
            &ctx,
            undo_id,
            &setup.api_key,
            &format!("/api/migration-jobs/{undo_id}"),
        )
        .await;

        let get_after_undo = request
            .get(&format!("/api/entities/{}", entity))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        let fetched_after_undo: serde_json::Value = get_after_undo.json();
        assert!(
            fetched_after_undo["data"].get("summary").is_none(),
            "undo must restore the pre-inference state: {fetched_after_undo:?}"
        );
    })
    .await;
}

/// Invalid proposals are marked invalid and do not leave a snapshot behind.
#[tokio::test]
async fn invalid_proposals_do_not_leave_a_snapshot() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let entity = create_entity(&request, &setup).await;

        let job_id = Uuid::new_v4();
        inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
            .await
            .expect("create proposal job");
        inference_jobs::claim(&ctx.db, job_id)
            .await
            .expect("claim proposal job");
        let txn = super::workspace_txn(&ctx, setup.tenant_id, setup.workspace_id).await;
        let schema =
            yorishiro::models::schema_schemas::get_active_schema(&txn, setup.workspace_id, "note")
                .await
                .expect("active schema");
        yorishiro::edition::ee::models::inference_proposals::record_batch(
            &txn,
            setup.workspace_id,
            job_id,
            schema.id,
            schema.version,
            [(entity, "summary".into(), serde_json::json!(12345))],
        )
        .await
        .expect("record invalid proposal");
        inference_jobs::complete(&txn, job_id, 0, 0)
            .await
            .expect("complete proposal job");
        let report = yorishiro::edition::ee::models::inference_proposals::confirm(
            &txn,
            setup.workspace_id,
            job_id,
            None,
        )
        .await
        .expect("confirm invalid proposal");
        assert_eq!(report.invalid, 1);

        // No snapshot should remain for this job_id.
        let remaining = yorishiro::models::_entities::entity_snapshots::Entity::find()
            .filter(
                yorishiro::models::_entities::entity_snapshots::Column::WorkspaceId
                    .eq(setup.workspace_id),
            )
            .filter(yorishiro::models::_entities::entity_snapshots::Column::JobId.eq(job_id))
            .count(&txn)
            .await
            .expect("count snapshots");
        assert_eq!(remaining, 0, "a rejected write must not leave a snapshot");

        txn.rollback().await.expect("rollback txn");
    })
    .await;
}

async fn create_completed_proposal(
    ctx: &loco_rs::app::AppContext,
    setup: &TestTenant,
    entity: Uuid,
    job_id: Uuid,
    value: serde_json::Value,
) {
    inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
        .await
        .expect("create proposal job");
    inference_jobs::claim(&ctx.db, job_id)
        .await
        .expect("claim proposal job");
    let txn = super::workspace_txn(ctx, setup.tenant_id, setup.workspace_id).await;
    let schema =
        yorishiro::models::schema_schemas::get_active_schema(&txn, setup.workspace_id, "note")
            .await
            .expect("active schema");
    yorishiro::edition::ee::models::inference_proposals::record_batch(
        &txn,
        setup.workspace_id,
        job_id,
        schema.id,
        schema.version,
        [(entity, "summary".into(), value)],
    )
    .await
    .expect("record proposal");
    inference_jobs::complete(&txn, job_id, 0, 0)
        .await
        .expect("complete proposal job");
    txn.commit().await.expect("commit proposal");
}

#[tokio::test]
async fn proposals_can_be_listed_rejected_and_discarded_only_in_their_workspace() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let entity = create_entity(&request, &setup).await;
        let rejected_job = Uuid::new_v4();
        let discarded_job = Uuid::new_v4();
        create_completed_proposal(
            &ctx,
            &setup,
            entity,
            rejected_job,
            serde_json::json!("reject me"),
        )
        .await;
        create_completed_proposal(
            &ctx,
            &setup,
            entity,
            discarded_job,
            serde_json::json!("discard me"),
        )
        .await;

        let listed = request
            .get(&format!("/api/inference-jobs/{rejected_job}/proposals"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            listed.status_code(),
            StatusCode::OK,
            "response: {:?}",
            listed.text()
        );
        let proposals: serde_json::Value = listed.json();
        assert_eq!(proposals.as_array().unwrap().len(), 1);
        assert_eq!(proposals[0]["status"], "pending");
        assert_eq!(proposals[0]["proposed"], "reject me");

        let rejected = request
            .post(&format!("/api/inference-jobs/{rejected_job}/reject"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            rejected.status_code(),
            StatusCode::OK,
            "response: {:?}",
            rejected.text()
        );
        assert_eq!(rejected.json::<serde_json::Value>()["changed"], 1);

        let discarded = request
            .post(&format!("/api/inference-jobs/{discarded_job}/discard"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            discarded.status_code(),
            StatusCode::OK,
            "response: {:?}",
            discarded.text()
        );
        assert_eq!(discarded.json::<serde_json::Value>()["changed"], 1);

        // SQLite permits one tenant; cross-tenant authorization belongs to PostgreSQL.
        if !crate::require_postgres_backend() {
            return;
        }
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let denied = request
            .get(&format!("/api/inference-jobs/{rejected_job}/proposals"))
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await;
        assert_eq!(
            denied.status_code(),
            StatusCode::FORBIDDEN,
            "response: {:?}",
            denied.text()
        );
    })
    .await;
}

/// A terminal action that wins the per-job lock must prevent an overlapping confirmation from
/// writing the entity. This uses two real PostgreSQL transactions rather than a sequential replay.
#[tokio::test]
async fn terminal_proposal_actions_serialize_against_confirmation() {
    // It holds an advisory lock across two transactions, which SQLite has no equivalent of.
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let entity = create_entity(&request, &setup).await;
        let db = ctx.shared_store.get::<DbHandle>().unwrap().clone();

        for terminal_action in ["reject", "discard"] {
            let job_id = Uuid::new_v4();
            create_completed_proposal(
                &ctx,
                &setup,
                entity,
                job_id,
                serde_json::json!(format!("must not apply: {terminal_action}")),
            )
            .await;

            let terminal_db = db.clone();
            let confirm_db = db.clone();
            let terminal_setup = setup.workspace_id;
            let terminal_tenant = setup.tenant_id;
            let confirm_tenant = setup.tenant_id;
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let confirm_barrier = barrier.clone();
            // Hold the action lock before starting confirmation. The confirmation races in a
            // separate task, but cannot pass the lock until this terminal action commits.
            let terminal_txn = terminal_db
                .tenant
                .begin_for_workspace(terminal_tenant, terminal_setup)
                .await
                .expect("begin terminal proposal transaction");
            yorishiro::db::lock_for_update(&terminal_txn, &format!("inference-proposals:{job_id}"))
                .await
                .expect("hold proposal action lock");
            let confirm = tokio::spawn(async move {
                let txn = confirm_db
                    .tenant
                    .begin_for_workspace(confirm_tenant, terminal_setup)
                    .await
                    .expect("begin confirmation transaction");
                confirm_barrier.wait().await;
                let result = yorishiro::edition::ee::models::inference_proposals::confirm(
                    &txn,
                    terminal_setup,
                    job_id,
                    None,
                )
                .await;
                if result.is_ok() {
                    txn.commit().await.expect("commit confirmation");
                } else {
                    txn.rollback().await.expect("rollback confirmation");
                }
                result
            });
            barrier.wait().await;
            let terminal_result = if terminal_action == "reject" {
                yorishiro::edition::ee::models::inference_proposals::reject(
                    &terminal_txn,
                    terminal_setup,
                    job_id,
                )
                .await
            } else {
                yorishiro::edition::ee::models::inference_proposals::discard(
                    &terminal_txn,
                    terminal_setup,
                    job_id,
                )
                .await
            }
            .expect("terminal proposal action");
            terminal_txn.commit().await.expect("commit terminal action");
            let confirm_result = confirm.await.expect("join confirmation task");

            assert_eq!(terminal_result.changed, 1);
            assert!(
                confirm_result.is_err(),
                "confirmation must observe the terminal {terminal_action}"
            );

            let entity_after =
                yorishiro::models::entity_entities::get(&ctx.db, setup.workspace_id, entity)
                    .await
                    .expect("read entity after race");
            assert_eq!(entity_after.data["summary"], serde_json::Value::Null);
            let proposals = yorishiro::edition::ee::models::inference_proposals::for_job(
                &ctx.db,
                setup.workspace_id,
                job_id,
            )
            .await
            .expect("read terminal proposal");
            assert_eq!(
                proposals[0].status.to_string(),
                if terminal_action == "reject" {
                    "rejected"
                } else {
                    "discarded"
                }
            );
        }
    })
    .await;
}

#[tokio::test]
async fn incomplete_and_failed_jobs_cannot_confirm_proposals() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let entity = create_entity(&request, &setup).await;
        let queued_job = Uuid::new_v4();
        create_pending_proposal(&ctx, &setup, entity, queued_job).await;
        let queued = request
            .post(&format!("/api/inference-jobs/{queued_job}/confirm"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            queued.status_code(),
            StatusCode::CONFLICT,
            "response: {:?}",
            queued.text()
        );

        // A workspace runs one job at a time, so the first one has to stop before another can start.
        inference_jobs::fail(&ctx.db, queued_job, "stopped")
            .await
            .expect("stop the first job");
        let failed_job = Uuid::new_v4();
        create_pending_proposal(&ctx, &setup, entity, failed_job).await;
        inference_jobs::fail(&ctx.db, failed_job, "provider failed")
            .await
            .expect("fail proposal job");
        let failed = request
            .post(&format!("/api/inference-jobs/{failed_job}/confirm"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            failed.status_code(),
            StatusCode::CONFLICT,
            "response: {:?}",
            failed.text()
        );

        let listed = request
            .get(&format!("/api/inference-jobs/{failed_job}/proposals"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        let proposals: serde_json::Value = listed.json();
        assert_eq!(proposals[0]["status"], "discarded");
    })
    .await;
}

async fn create_pending_proposal(
    ctx: &loco_rs::app::AppContext,
    setup: &TestTenant,
    entity: Uuid,
    job_id: Uuid,
) {
    inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
        .await
        .expect("create proposal job");
    inference_jobs::claim(&ctx.db, job_id)
        .await
        .expect("claim proposal job");
    let txn = super::workspace_txn(ctx, setup.tenant_id, setup.workspace_id).await;
    let schema =
        yorishiro::models::schema_schemas::get_active_schema(&txn, setup.workspace_id, "note")
            .await
            .expect("active schema");
    yorishiro::edition::ee::models::inference_proposals::record_batch(
        &txn,
        setup.workspace_id,
        job_id,
        schema.id,
        schema.version,
        [(entity, "summary".into(), serde_json::json!("pending"))],
    )
    .await
    .expect("record proposal");
    txn.commit().await.expect("commit proposal");
}

/// `infer_job_status` is registered on its own path (`GET /api/inference-jobs/{job_id}`),
/// not co-registered on `POST /api/schemas/active/{name}/infer-fill` where axum's router
/// would have handed the schema name to a handler that expects a `job_id`.
///
/// This test posts to `infer-fill`, collects the `job_id` from the response, and polls the
/// dedicated status endpoint with that `job_id`, confirming the status route resolves to a
/// 200 (the durable status row stores the job and returns it) rather than a 404 or a misrouted
/// response.
#[tokio::test]
async fn infer_job_status_is_on_its_own_path_not_colliding_with_infer_fill() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        // Set up a workspace LLM key so infer-fill passes its validation gate.
        let put_key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1",
                "model": "gpt-4o-mini",
                "api_key": "sk-test-key"
            }))
            .await;
        assert_eq!(
            put_key.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            put_key.text()
        );

        // Create a schema with at least one entity so infer-fill has work to do.
        let create_schema = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "name": "article",
                "entity_types": {
                    "article": {
                        "fields": {
                            "title": { "type": "string", "required": true },
                            "body": { "type": "string" }
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "schema_name": "article",
                "entity_type": "article",
                "data": { "title": "hello", "body": "world" }
            }))
            .await;
        assert_eq!(
            create_entity.status_code(),
            201,
            "response: {:?}",
            create_entity.text()
        );

        // POST to infer-fill — the model will reject this (no real LLM), but the handler
        // returns a job_id even when the queue is busy or the model is unreachable; we only
        // need a real job_id to prove the status route is on a different path.
        let infer = request
            .post("/api/schemas/active/article/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            infer.status_code(),
            200,
            "infer-fill must return a job_id: {:?}",
            infer.text()
        );
        let infer_body: serde_json::Value = infer.json();
        assert_eq!(infer_body["status"], "queued");
        let job_id = infer_body["job_id"]
            .as_str()
            .expect("response must contain a job_id string");

        // Poll the dedicated status endpoint with that real job_id.
        // Before the fix this would have been a 404 because the route was on
        // `/active/{name}/infer-fill` (the POST path), not `/inference-jobs/{job_id}`.
        let status = request
            .get(&format!("/api/inference-jobs/{job_id}"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            status.status_code(),
            200,
            "status must resolve on /api/inference-jobs/{{job_id}}: {:?}",
            status.text()
        );
        let status_body: serde_json::Value = status.json();
        assert_eq!(status_body["job_id"].as_str().unwrap(), job_id);

        // Verify the POST and GET handlers are distinct: a GET on the infer-fill path
        // should not be handled by infer_job_status (it should be a 405 Method Not Allowed
        // or a 404, depending on how axum resolves it — the point is it's not the job
        // status handler).
        let get_on_infer_fill = request
            .get("/api/schemas/active/test/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert!(
            get_on_infer_fill.status_code() != 200
                || get_on_infer_fill.json::<serde_json::Value>()["job_id"]
                    != serde_json::json!("test"),
            "the POST path must not serve job status"
        );
    })
    .await;
}

/// A dispatch failure rolls back the job together with its durable dispatch records.
#[tokio::test]
async fn infer_fill_dispatch_failure_rolls_back_job() {
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let put_key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1",
                "model": "gpt-4o-mini",
                "api_key": "sk-test-key"
            }))
            .await;
        assert_eq!(put_key.status_code(), StatusCode::NO_CONTENT);

        use sea_orm::{ConnectionTrait, DbBackend};
        // Database fault injection is isolated from other tests and uses real enqueue.
        let fault = match ctx.db.get_database_backend() {
            DbBackend::Postgres => "ALTER TABLE queue_job_dispatch_outbox ADD CONSTRAINT refuse_test_dispatch CHECK (false)",
            DbBackend::Sqlite => "CREATE TRIGGER refuse_test_dispatch BEFORE INSERT ON queue_job_dispatch_outbox BEGIN SELECT RAISE(ABORT, 'refuse_test_dispatch'); END",
            backend => panic!("unsupported test backend: {backend:?}"),
        };
        ctx.db.execute_unprepared(fault).await.unwrap();

        let response = request
            .post("/api/schemas/active/article/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(response.status_code(), StatusCode::INTERNAL_SERVER_ERROR);

        // The failed POST cannot return a job ID; inspect this workspace for orphan rows.
        use yorishiro::models::_entities::inference_jobs as rows;
        let jobs = rows::Entity::find()
            .filter(rows::Column::WorkspaceId.eq(setup.workspace_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert!(jobs.is_empty(), "failed enqueue must not leave an orphan job");
    })
    .await;
}

#[tokio::test]
async fn inference_status_polling_preserves_all_wire_values() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let mut jobs = Vec::new();

        let queued = Uuid::new_v4();
        inference_jobs::create(&ctx.db, queued, setup.workspace_id, "notes")
            .await
            .expect("create queued job");
        jobs.push((queued, InferenceJobStatus::Queued));

        let completed = Uuid::new_v4();
        inference_jobs::create(&ctx.db, completed, setup.workspace_id, "notes")
            .await
            .expect("create completed job");
        assert!(inference_jobs::claim(&ctx.db, completed).await.unwrap());
        inference_jobs::complete(&ctx.db, completed, 3, 1)
            .await
            .expect("complete job");
        jobs.push((completed, InferenceJobStatus::Completed));

        // Last of the claims: a workspace holds one running job at a time.
        let running = Uuid::new_v4();
        inference_jobs::create(&ctx.db, running, setup.workspace_id, "notes")
            .await
            .expect("create running job");
        assert!(inference_jobs::claim(&ctx.db, running).await.unwrap());
        jobs.push((running, InferenceJobStatus::Running));

        let failed = Uuid::new_v4();
        inference_jobs::create(&ctx.db, failed, setup.workspace_id, "notes")
            .await
            .expect("create failed job");
        inference_jobs::fail(&ctx.db, failed, "provider failed")
            .await
            .expect("fail job");
        jobs.push((failed, InferenceJobStatus::Failed));

        for (job_id, expected) in jobs {
            let response = request
                .get(&format!("/api/inference-jobs/{job_id}"))
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .await;
            assert_eq!(
                response.status_code(),
                StatusCode::OK,
                "response: {:?}",
                response.text()
            );
            let body: serde_json::Value = response.json();
            assert_eq!(body["status"], expected.as_db_str());
            if expected == InferenceJobStatus::Completed {
                assert_eq!(body["applied"], 3);
                assert_eq!(body["skipped"], 1);
            } else if expected == InferenceJobStatus::Failed {
                assert_eq!(body["error"], "provider failed");
                assert!(body["applied"].is_null());
                assert!(body["skipped"].is_null());
            } else {
                assert!(body["applied"].is_null());
                assert!(body["skipped"].is_null());
            }
        }
    })
    .await;
}

/// The worker runs outside any request and the queued job names only the workspace, so it finds the tenant itself; the two ids differ here.
#[tokio::test]
async fn the_infer_fill_worker_runs_against_a_workspace_that_is_not_its_tenant() {
    let (provider, _server, _) = crate::http_stub::keyword_provider(768).await;
    super::query_worker::boot_with_query_worker(provider, |request, ctx| async move {
        require_licence(&ctx);
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert_ne!(setup.tenant_id, setup.workspace_id);
        create_entity(&request, &setup).await;
        let key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1/",
                "model": "gpt-4o-mini",
                "api_key": "sk-secret-value"
            }))
            .await;
        assert_eq!(key.status_code(), StatusCode::NO_CONTENT);

        // Every entity is already on the active schema, so the run reaches the schema read and needs no LLM call.
        let response = request
            .post("/api/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        response.assert_status(StatusCode::OK);
        let job_id = response.json::<serde_json::Value>()["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let response = request
                    .get(&format!("/api/inference-jobs/{job_id}"))
                    .add_header("Authorization", format!("Bearer {}", setup.api_key))
                    .await;
                response.assert_status(StatusCode::OK);
                let job = response.json::<serde_json::Value>();
                if job["status"] == "completed" {
                    break job;
                }
                assert_ne!(job["status"], "failed", "{job}");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("real worker resolves the workspace's tenant and completes");
        assert!(completed["error"].is_null(), "{completed}");
    })
    .await;
}

#[tokio::test]
async fn the_infer_fill_scope_carries_the_workspaces_tenant() {
    use sea_orm::{ConnectionTrait, Statement};
    use yorishiro::edition::ee::workers::infer_fill::open_workspace_scope;

    // The settings are PostgreSQL session settings that row-level security reads.
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert_ne!(setup.tenant_id, setup.workspace_id);

        let txn = open_workspace_scope(&ctx, setup.workspace_id)
            .await
            .expect("open the scope");
        let row = txn
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT current_setting('app.current_tenant') AS tenant, current_setting('app.current_workspace') AS workspace",
            ))
            .await
            .expect("read the settings")
            .expect("one row");
        let tenant: String = row.try_get("", "tenant").unwrap();
        let workspace: String = row.try_get("", "workspace").unwrap();
        assert_eq!(tenant, setup.tenant_id.to_string());
        assert_eq!(workspace, setup.workspace_id.to_string());
        txn.rollback().await.expect("release the worker scope");

        let unknown = open_workspace_scope(&ctx, Uuid::new_v4()).await;
        assert!(unknown.is_err(), "a workspace that does not exist has no scope");
    })
    .await;
}
