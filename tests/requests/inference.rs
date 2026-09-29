use super::boot_request;
use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::db::DbHandle;
use yorishiro::ee::models::inference_jobs::{self, InferenceJobStatus};
use yorishiro::ee::services::licence::{LicenceClaims, LicenceState};
use yorishiro::ee::workers::infer_fill::{InferFillArgs, TestInferFillDispatcher};
use yorishiro::models::_entities::{api_keys, tenant_tenants, workspace_workspaces};
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::auth::ApiKeyScope;

/// `shared_store.insert` is keyed by `TypeId`, so this overwrites the enterprise-edition state the test process booted with.
/// See `marketplace.rs`'s own copy of this helper.
fn licence(ctx: &loco_rs::app::AppContext) {
    ctx.shared_store
        .insert(std::sync::Arc::new(LicenceState::licensed(LicenceClaims {
            sub: "acme-corp".into(),
            plan: "enterprise".into(),
            exp: Utc::now().timestamp() + 60 * 60,
        }))
            as std::sync::Arc<
                dyn yorishiro::services::edition::EnterpriseEdition,
            >);
}

struct Setup {
    key: String,
    tenant_id: Uuid,
    workspace_id: Uuid,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
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
    let key = api_keys::Entity::create_api_key(
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
async fn llm_key_set_get_and_clear_round_trip() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;

        let missing = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            missing.status_code(),
            StatusCode::NOT_FOUND,
            "response: {:?}",
            missing.text()
        );

        let put = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            delete.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            delete.text()
        );

        let after_delete = request
            .get("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(after_delete.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// A scheme that could never be a chat-completions endpoint is refused before anything is stored, and a URL with no scheme at all is refused too rather than becoming a relative path.
#[tokio::test]
async fn a_non_http_base_url_is_refused() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
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
    })
    .await;
}

/// A workspace with no credentials configured is refused with one clear error before any entity is scanned, rather than reporting zero applied in a way that reads as "nothing to infer".
#[tokio::test]
async fn infer_fill_without_a_configured_key_is_refused() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
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
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let with_key = request
            .post("/api/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let without_key = request.post("/api/schemas/active/note/infer-fill").await;

        assert_eq!(with_key.status_code(), without_key.status_code());
        assert_eq!(with_key.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// Creates one schema and one entity for the infer-fill request tests.
async fn create_entity(request: &axum_test::TestServer, setup: &Setup) -> Uuid {
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

    entity_id
}

/// A proposal remains separate from the entity until an explicit confirmation.
#[tokio::test]
async fn proposals_require_explicit_confirmation_and_undo_reverses_it() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;
        let entity = create_entity(&request, &setup).await;

        let db = ctx.shared_store.get::<DbHandle>().unwrap();
        let job_id = Uuid::new_v4();
        inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
            .await
            .expect("create proposal job");
        {
            let txn = db
                .tenant
                .begin_for_workspace(setup.tenant_id, setup.workspace_id)
                .await
                .expect("begin tenant txn");
            let schema = yorishiro::models::schema_schemas::get_active_schema(
                &txn,
                setup.workspace_id,
                "note",
            )
            .await
            .expect("active schema");
            yorishiro::ee::models::inference_proposals::record_batch(
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
            txn.commit().await.expect("commit apply");
        }

        let get_entity = request
            .get(&format!("/api/entities/{}", entity))
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            confirm.status_code(),
            StatusCode::OK,
            "response: {:?}",
            confirm.text()
        );

        // The accepted write is snapshot-backed and remains undoable through the base endpoint.
        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            undo.status_code(),
            StatusCode::OK,
            "response: {:?}",
            undo.text()
        );
        let undo_report: serde_json::Value = undo.json();
        assert_eq!(undo_report["restored"], 1, "undo report: {undo_report:?}");

        let get_after_undo = request
            .get(&format!("/api/entities/{}", entity))
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;
        let entity = create_entity(&request, &setup).await;

        let db = ctx.shared_store.get::<DbHandle>().unwrap();
        let job_id = Uuid::new_v4();
        inference_jobs::create(&ctx.db, job_id, setup.workspace_id, "note")
            .await
            .expect("create proposal job");
        let txn = db
            .tenant
            .begin_for_workspace(setup.tenant_id, setup.workspace_id)
            .await
            .expect("begin tenant txn");
        let schema =
            yorishiro::models::schema_schemas::get_active_schema(&txn, setup.workspace_id, "note")
                .await
                .expect("active schema");
        yorishiro::ee::models::inference_proposals::record_batch(
            &txn,
            setup.workspace_id,
            job_id,
            schema.id,
            schema.version,
            [(entity, "summary".into(), serde_json::json!(12345))],
        )
        .await
        .expect("record invalid proposal");
        let report = yorishiro::ee::models::inference_proposals::confirm(
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
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;

        // Set up a workspace LLM key so infer-fill passes its validation gate.
        let put_key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
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

struct FailingInferFillDispatcher {
    args: Mutex<Option<InferFillArgs>>,
}

#[async_trait]
impl TestInferFillDispatcher for FailingInferFillDispatcher {
    async fn dispatch(
        &self,
        _ctx: &loco_rs::app::AppContext,
        args: InferFillArgs,
    ) -> loco_rs::Result<String> {
        *self.args.lock().unwrap() = Some(args);
        Err(loco_rs::Error::Message(
            "fake infer-fill dispatch failure".into(),
        ))
    }
}

/// A dispatch failure leaves the durable infer-fill row failed rather than queued forever.
#[tokio::test]
async fn infer_fill_dispatch_failure_persists_failed_job() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;
        let put_key = request
            .put("/api/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1",
                "model": "gpt-4o-mini",
                "api_key": "sk-test-key"
            }))
            .await;
        assert_eq!(put_key.status_code(), StatusCode::NO_CONTENT);

        let dispatcher = Arc::new(FailingInferFillDispatcher {
            args: Mutex::new(None),
        });
        yorishiro::ee::workers::infer_fill::install_test_infer_fill_dispatcher(
            &ctx,
            dispatcher.clone(),
        );

        let response = request
            .post("/api/schemas/active/article/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(response.status_code(), StatusCode::INTERNAL_SERVER_ERROR);

        let args = dispatcher
            .args
            .lock()
            .unwrap()
            .clone()
            .expect("dispatcher must receive infer-fill args");
        let job = inference_jobs::get(&ctx.db, args.job_id)
            .await
            .expect("read failed infer-fill job")
            .expect("durable infer-fill job must exist");
        assert_eq!(job.workspace_id, setup.workspace_id);
        assert_eq!(job.schema_name, "article");
        assert_eq!(job.status, InferenceJobStatus::Failed);
        assert_eq!(
            job.error.as_deref(),
            Some("failed to enqueue infer-fill job: fake infer-fill dispatch failure")
        );
    })
    .await;
}

#[tokio::test]
async fn inference_status_polling_preserves_all_wire_values() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;
        let mut jobs = Vec::new();

        let queued = Uuid::new_v4();
        inference_jobs::create(&ctx.db, queued, setup.workspace_id, "notes")
            .await
            .expect("create queued job");
        jobs.push((queued, InferenceJobStatus::Queued));

        let running = Uuid::new_v4();
        inference_jobs::create(&ctx.db, running, setup.workspace_id, "notes")
            .await
            .expect("create running job");
        assert!(inference_jobs::claim(&ctx.db, running).await.unwrap());
        jobs.push((running, InferenceJobStatus::Running));

        let completed = Uuid::new_v4();
        inference_jobs::create(&ctx.db, completed, setup.workspace_id, "notes")
            .await
            .expect("create completed job");
        assert!(inference_jobs::claim(&ctx.db, completed).await.unwrap());
        inference_jobs::complete(&ctx.db, completed, 3, 1)
            .await
            .expect("complete job");
        jobs.push((completed, InferenceJobStatus::Completed));

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
                .add_header("Authorization", format!("Bearer {}", setup.key))
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
