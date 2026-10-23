use std::time::Duration;

use futures::FutureExt;
use loco_rs::app::{AppContext, Hooks};
use sea_orm::{ConnectionTrait, EntityTrait};
use uuid::Uuid;
use yorishiro::workers::target::WorkerTarget;

pub(crate) use super::boot_request_isolated as boot_request;

#[cfg(feature = "enterprise")]
pub(crate) async fn finish(ctx: &AppContext, id: Uuid) {
    finish_with_status(ctx, id, "completed").await;
}

#[cfg(feature = "enterprise")]
pub(crate) async fn finish_with_status(ctx: &AppContext, id: Uuid, expected: &str) {
    finish_job(ctx, id, expected, None).await;
}

pub(crate) async fn finish_via_api(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    id: Uuid,
    key: &str,
    status_path: &str,
) {
    finish_job(ctx, id, "completed", Some((request, key, status_path))).await;
}

#[cfg(feature = "enterprise")]
pub(crate) async fn fail_via_api(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    id: Uuid,
    key: &str,
    status_path: &str,
) {
    finish_job(ctx, id, "failed", Some((request, key, status_path))).await;
}

async fn finish_job(
    ctx: &AppContext,
    id: Uuid,
    expected: &str,
    status_api: Option<(&loco_rs::TestServer, &str, &str)>,
) {
    let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let tag = outbox.tags.unwrap()[0].as_str().unwrap().to_owned();
    let target = WorkerTarget::parse_tag(&tag).unwrap();
    let queue = super::query_worker::target_queue(ctx, target).await;
    let mut worker_ctx = ctx.clone();
    worker_ctx.queue_provider = Some(queue.clone());
    yorishiro::App::connect_workers(&worker_ctx, &queue)
        .await
        .expect("register production workers");
    let running = queue.clone();
    let worker = tokio::spawn(async move { running.run(vec![target.tag()]).await });
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let status = if let Some((request, key, path)) = status_api {
                    let response = request
                        .get(path)
                        .add_header("Authorization", format!("Bearer {key}"))
                        .await;
                    response.assert_status(axum::http::StatusCode::OK);
                    response.json::<serde_json::Value>()["status"]
                        .as_str()
                        .expect("job status")
                        .to_owned()
                } else {
                    yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
                        .one(&ctx.db)
                        .await
                        .unwrap()
                        .unwrap()
                        .status
                };
                if matches!(status.as_str(), "completed" | "failed") {
                    assert_eq!(status, expected, "worker outcome");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }))
        .catch_unwind()
        .await;
    if matches!(&result, Ok(Err(_))) {
        let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
            .one(&ctx.db)
            .await;
        let dispatch =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
                .one(&ctx.db)
                .await;
        eprintln!(
            "worker timeout: lifecycle={:?}, dispatch={:?}",
            lifecycle.map(|row| row.map(|row| (row.status, row.attempt, row.provider_job_id))),
            dispatch.map(|row| row.map(|row| (
                row.published_at,
                row.last_attempt_at,
                row.claim_until
            ))),
        );
        let directory = tempfile::tempdir().expect("queue diagnostic directory");
        match queue.dump(directory.path(), None, None).await {
            Ok(path) => {
                let jobs: serde_yaml::Value = serde_yaml::from_str(
                    &std::fs::read_to_string(path).expect("read queue diagnostics"),
                )
                .expect("decode queue diagnostics");
                if let Some(jobs) = jobs.as_sequence() {
                    for job in jobs {
                        eprintln!(
                            "worker timeout queue job: id={:?}, name={:?}, status={:?}",
                            job.get("id"),
                            job.get("name"),
                            job.get("status")
                        );
                    }
                }
            }
            Err(error) => eprintln!("worker timeout queue inspection failed: {error}"),
        }
        if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
            let activity = ctx.db.query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT COALESCE(jsonb_agg(jsonb_build_object('pid', pid, 'state', state, 'wait_type', wait_event_type, 'wait', wait_event, 'blockers', pg_blocking_pids(pid))), '[]'::jsonb) AS activity FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid()",
            )).await.map(|row| row.map(|row| row.try_get::<serde_json::Value>("", "activity")));
            eprintln!("worker timeout database activity: {activity:?}");
        }
    }
    yorishiro::App::on_shutdown(&worker_ctx).await;
    let shutdown = queue.shutdown();
    let joined = tokio::time::timeout(Duration::from_secs(10), worker).await;
    let closed = queue.close().await;
    match result {
        Ok(outcome) => outcome.expect("migration must complete on the configured queue provider"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    shutdown.expect("shutdown worker queue");
    joined
        .expect("drain worker")
        .expect("join worker")
        .expect("run worker");
    closed.expect("close drained worker queue");
}

#[tokio::test]
async fn rest_fill_defaults_waits_for_a_loco_worker() {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{api_keys::ApiKeyScope, entity_entities, schema_schemas};

    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let super::fixtures::TestTenant { tenant_id: tenant, workspace_id: workspace, api_key: key, .. } = super::fixtures::TestTenant::create(&request, &ctx, super::fixtures::TenantArgs { key_scope: ApiKeyScope::Migration, ..Default::default() },
        ).await;
        let mut entity_id = None;
        for (version, fields) in [json!({"title": {"type": "string", "required": true}}),
            json!({"title": {"type": "string", "required": true}, "rank": {"type": "integer", "required": true, "default": 7}})]
            .into_iter().enumerate()
        {
            schema_schemas::create_schema(&ctx.db, tenant, workspace,
                serde_json::from_value(json!({"name": "note", "entity_types": {"note": {"fields": fields}}})).unwrap(),
                None, None).await.unwrap();
            if version == 0 {
                entity_id = Some(entity_entities::create(&ctx.db, workspace, entity_entities::CreateEntityInput {
                    schema_name: "note".into(), entity_type: "note".into(), data: json!({"title": "before"}),
                }, None).await.unwrap().id);
            }
        }
        let response = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&json!({"schema_name": "note"})).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED, "{}", response.text());
        let id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        let entity_id = entity_id.unwrap();
        let before = entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap();
        assert_eq!(before.data, json!({"title": "before"}));
        finish_via_api(&request, &ctx, id, &key, &format!("/api/migration-jobs/{id}")).await;
        let after = entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap();
        assert_eq!(after.data, json!({"title": "before", "rank": 7}));
    }).await;
}
