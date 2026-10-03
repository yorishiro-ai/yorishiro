//! The queue-lifecycle protocol every class-restricted worker follows.
//!
//! A job that carries a `lifecycle_id` is admitted against `queue_job_lifecycles` before it runs, keeps its lease alive while it runs, and is recorded as completed or deferred afterwards.
//! A job without one (enqueued before lifecycle tracking existed) just runs.

use std::future::Future;

use loco_rs::{app::AppContext, bgworker::BackgroundWorker};
use serde::Serialize;
use uuid::Uuid;

use crate::models::queue_job_lifecycles::{Admission, Entity, LifecycleStatus};
use crate::workers::embedding_sync::WorkerClass;

const SATURATED: &str = "worker capacity saturated";

/// Runs `work` under the lifecycle `lifecycle_id` names, re-enqueueing the job through `W` when it has to wait or fails.
///
/// A failure is recorded and re-enqueued instead of returned: Loco has no retry of its own, so returning `Err` would drop the job from the queue while the lifecycle row still said it was waiting.
/// The re-enqueue goes through `W` so the retry keeps the tag of the class that owns the job.
pub(crate) async fn perform_with_lifecycle<W, A, F, Fut>(
    ctx: &AppContext,
    lifecycle_id: Option<Uuid>,
    class: WorkerClass,
    args: &A,
    work: F,
) -> loco_rs::Result<()>
where
    W: BackgroundWorker<A>,
    A: Clone + Send + Sync + Serialize + 'static,
    F: FnOnce() -> Fut,
    Fut: Future<Output = loco_rs::Result<()>>,
{
    let Some(id) = lifecycle_id else {
        return work().await;
    };

    let attempt = match Entity::start(&ctx.db, id).await.map_err(message)? {
        Admission::Started { attempt } | Admission::Recovered { attempt } => attempt,
        Admission::Duplicate { .. } | Admission::Terminal => return Ok(()),
        Admission::Saturated { attempt } => {
            let scheduling = crate::services::queue::decide(class);
            tracing::warn!(
                lifecycle_id = %id,
                worker_class = class.as_db_str(),
                scheduling_priority = scheduling.priority,
                fallback = scheduling.fallback,
                "worker capacity saturated; preserving class reservation"
            );
            // An attempt means an expired lease was reclaimed, so the deferral is fenced by that attempt and by the lease having lapsed.
            match attempt {
                Some(attempt) => {
                    Entity::defer_at(
                        &ctx.db,
                        id,
                        attempt,
                        SATURATED,
                        chrono::Utc::now().fixed_offset(),
                    )
                    .await
                }
                None => Entity::defer(&ctx.db, id, None, SATURATED).await,
            }
            .map_err(message)?;
            return requeue::<W, A>(ctx, class, args).await;
        }
    };

    let heartbeat = Entity::heartbeat(ctx.db.clone(), id, attempt);
    let result = work().await;
    heartbeat.abort();

    match result {
        Ok(()) => {
            if let Err(error) =
                Entity::finish(&ctx.db, id, Some(attempt), LifecycleStatus::Completed, None).await
            {
                tracing::warn!(lifecycle_id = %id, diagnostic = %error, "job finished but its lifecycle could not be completed");
            }
            Ok(())
        }
        Err(error) => {
            Entity::defer(&ctx.db, id, Some(attempt), &error.to_string())
                .await
                .map_err(message)?;
            requeue::<W, A>(ctx, class, args).await
        }
    }
}

async fn requeue<W, A>(ctx: &AppContext, class: WorkerClass, args: &A) -> loco_rs::Result<()>
where
    W: BackgroundWorker<A>,
    A: Clone + Send + Sync + Serialize + 'static,
{
    let scheduling = crate::services::queue::decide(class);
    W::perform_later_with_priority(ctx, args.clone(), Some(scheduling.priority)).await?;
    Ok(())
}

fn message(error: sea_orm::DbErr) -> loco_rs::Error {
    loco_rs::Error::Message(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use loco_rs::environment::Environment;
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{Database, EntityTrait};
    use tempfile::tempdir;

    use super::*;
    use crate::models::queue_job_lifecycles::Enqueue;
    use crate::workers::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorkerShared};

    async fn context() -> AppContext {
        let template = crate::workers::dispatch::test_context().await;
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
}
