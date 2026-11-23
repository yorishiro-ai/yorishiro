use std::{sync::Arc, time::Duration};

use futures::FutureExt;
use loco_rs::app::{AppContext, Hooks};
use sea_orm::{ConnectionTrait, EntityTrait};
use uuid::Uuid;
use yorishiro::{App, workers::target::WorkerTarget};

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn finish_via_api(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    id: Uuid,
    key: &str,
    status_path: &str,
) {
    finish_job(ctx, id, "completed", Some((request, key, status_path))).await;
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn fail_via_api(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    id: Uuid,
    key: &str,
    status_path: &str,
) {
    finish_job(ctx, id, "failed", Some((request, key, status_path))).await;
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn finish_job(
    ctx: &AppContext,
    id: Uuid,
    expected: &str,
    status_api: Option<(&loco_rs::TestServer, &str, &str)>,
) {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    // Capture relay failures from spawned tasks; queue state alone cannot explain a missing publish.
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_test_writer()
                .with_ansi(false),
        )
        .with(
            tracing_subscriber::filter::Targets::new()
                .with_target("yorishiro::workers::dispatch", tracing::Level::WARN)
                .with_target("yorishiro::initializers::queue", tracing::Level::ERROR),
        )
        .try_init();
    let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let tag = outbox.tags.unwrap()[0].as_str().unwrap().to_owned();
    let mut worker_ctx = ctx.clone();
    let queue = queue_for_tag(&mut worker_ctx, tag.clone()).await;
    let running = queue.clone();
    let worker = tokio::spawn(async move { running.run(vec![tag]).await });
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
                    let directory = tempfile::tempdir().expect("queue result directory");
                    let path = queue.dump(directory.path(), None, None).await.unwrap();
                    let jobs: Vec<loco_rs::bgworker::Job> =
                        serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                    let expected_status = if expected == "failed" {
                        loco_rs::bgworker::JobStatus::Failed
                    } else {
                        loco_rs::bgworker::JobStatus::Completed
                    };
                    if jobs.iter().any(|job| {
                        job.data["lifecycle_id"] == id.to_string() && job.status == expected_status
                    }) {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }))
        .catch_unwind()
        .await;
    if matches!(&result, Ok(Err(_))) {
        log_worker_timeout(ctx, id, &queue).await;
    }
    App::on_shutdown(&worker_ctx).await;
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

async fn log_worker_timeout(ctx: &AppContext, id: Uuid, queue: &loco_rs::bgworker::Queue) {
    let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
        .one(&ctx.db)
        .await;
    let dispatch = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&ctx.db)
        .await;
    eprintln!(
        "worker timeout: lifecycle={:?}, dispatch={:?}",
        lifecycle.map(|row| row.map(|row| (row.status, row.attempt, row.provider_job_id))),
        dispatch.map(|row| row.map(|row| (row.published_at, row.last_attempt_at, row.claim_until))),
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

pub(crate) async fn queue_for_tag(
    ctx: &mut AppContext,
    tag: String,
) -> Arc<loco_rs::bgworker::Queue> {
    let mode = loco_rs::boot::StartMode::WorkerOnly { tags: vec![tag] };
    yorishiro::workers::start::configure(&mut ctx.config, &mode)
        .expect("configure production worker role");
    let queue = loco_rs::bgworker::create_queue_provider(&ctx.config)
        .await
        .expect("create worker queue")
        .expect("background queue mode");
    queue.setup().await.expect("set up worker queue");
    ctx.queue_provider = Some(queue.clone());
    let boot = loco_rs::boot::run_app::<App>(&mode, ctx.clone())
        .await
        .expect("boot production worker through Loco");
    *ctx = boot.app_context;
    queue
}

/// Configures and registers a shared-target consumer through Loco, without starting its loop.
///
/// A Redis worker polls only the queues its configuration names, which `yorishiro start --worker=...` fills in from the targets it serves.
/// Updates the worker context before building its queue and running Loco's worker boot hooks over the already migrated database.
pub(crate) async fn shared_target_queue(ctx: &mut AppContext) -> Arc<loco_rs::bgworker::Queue> {
    target_queue(ctx, WorkerTarget::Shared).await
}

pub(crate) async fn target_queue(
    ctx: &mut AppContext,
    target: WorkerTarget,
) -> Arc<loco_rs::bgworker::Queue> {
    queue_for_tag(ctx, target.queue()).await
}

#[cfg(feature = "enterprise")]
pub(crate) async fn job_target_queue(
    ctx: &mut AppContext,
    target: WorkerTarget,
) -> Arc<loco_rs::bgworker::Queue> {
    queue_for_tag(ctx, target.job_tag()).await
}
