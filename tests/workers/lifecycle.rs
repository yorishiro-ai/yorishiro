use loco_rs::app::AppContext;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;
use yorishiro::models::queue_job_lifecycles::Entity;
use yorishiro::workers::embedding_sync::WorkerClass;

use loco_rs::environment::Environment;
use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, EntityTrait};
use tempfile::tempdir;

use yorishiro::models::queue_job_lifecycles::Enqueue;
use yorishiro::workers::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorkerShared};
use yorishiro::workers::lifecycle::*;

async fn context() -> AppContext {
    let template = crate::workers::test_context().await;
    let path = tempdir().unwrap().keep().join("lifecycle.sqlite3");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    AppContext::builder(Environment::Test, db, template.config.clone()).build()
}

async fn queued(ctx: &AppContext) -> Uuid {
    let id = Uuid::now_v7();
    Entity::record_enqueue(
        &ctx.db,
        Enqueue {
            id,
            job_name: "lifecycle-test",
            worker_class: WorkerClass::Shared,
            workspace_id: None,
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
    )
    .await
    .unwrap();
    id
}

fn args(lifecycle_id: Option<Uuid>) -> EmbeddingSyncArgs {
    EmbeddingSyncArgs {
        lifecycle_id,
        workspace_id: Uuid::nil(),
        entity_id: Uuid::nil(),
        worker_class: WorkerClass::Shared,
    }
}

async fn status(ctx: &AppContext, id: Uuid) -> String {
    Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
        .status
}

async fn run(
    ctx: &AppContext,
    args: &EmbeddingSyncArgs,
    runs: &AtomicUsize,
    outcome: loco_rs::Result<()>,
) -> loco_rs::Result<()> {
    perform_with_lifecycle::<EmbeddingSyncWorkerShared, _, _, _>(
        ctx,
        args.lifecycle_id,
        WorkerClass::Shared,
        args,
        || async {
            runs.fetch_add(1, Ordering::SeqCst);
            outcome
        },
    )
    .await
}

#[tokio::test]
async fn a_job_without_a_lifecycle_just_runs_and_returns_its_result() {
    let ctx = context().await;
    let runs = AtomicUsize::new(0);

    run(&ctx, &args(None), &runs, Ok(())).await.unwrap();
    let failed = run(
        &ctx,
        &args(None),
        &runs,
        Err(loco_rs::Error::string("boom")),
    )
    .await;

    assert!(failed.is_err());
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn success_completes_the_lifecycle_and_a_redelivery_does_not_run_again() {
    let ctx = context().await;
    let id = queued(&ctx).await;
    let runs = AtomicUsize::new(0);

    run(&ctx, &args(Some(id)), &runs, Ok(())).await.unwrap();
    assert_eq!(status(&ctx, id).await, "completed");

    run(&ctx, &args(Some(id)), &runs, Ok(())).await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failure_is_recorded_as_retrying_before_the_job_is_requeued() {
    let ctx = context().await;
    let id = queued(&ctx).await;
    let runs = AtomicUsize::new(0);

    // The test context has no queue provider, so the requeue itself fails: the lifecycle row must already say the job is waiting, and the failure must surface rather than vanish.
    let result = run(
        &ctx,
        &args(Some(id)),
        &runs,
        Err(loco_rs::Error::string("boom")),
    )
    .await;

    assert!(result.is_err());
    assert_eq!(status(&ctx, id).await, "retrying");
}
