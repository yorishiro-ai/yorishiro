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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn perform_with_lifecycle<W, A, F, Fut>(
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
            let scheduling = crate::workers::queue::decide(class);
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

/// Runs a job whose failures are deterministic and must not be immediately requeued.
///
/// Reindex provider and validation failures are terminal for this queued attempt;
/// operators can correct the provider or explicitly request another reindex.
///
/// # Errors
/// Returns an error if lifecycle persistence fails.
pub(crate) async fn perform_with_terminal_lifecycle<W, A, F, Fut>(
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
            Entity::finish(&ctx.db, id, Some(attempt), LifecycleStatus::Completed, None)
                .await
                .map_err(message)?;
            Ok(())
        }
        Err(error) => {
            Entity::finish(
                &ctx.db,
                id,
                Some(attempt),
                LifecycleStatus::Failed,
                Some(&error.to_string()),
            )
            .await
            .map_err(message)?;
            tracing::error!(lifecycle_id = %id, worker_class = class.as_db_str(), error = %error, "terminal worker failure");
            Ok(())
        }
    }
}

async fn requeue<W, A>(ctx: &AppContext, class: WorkerClass, args: &A) -> loco_rs::Result<()>
where
    W: BackgroundWorker<A>,
    A: Clone + Send + Sync + Serialize + 'static,
{
    let scheduling = crate::workers::queue::decide(class);
    W::perform_later_with_priority(ctx, args.clone(), Some(scheduling.priority)).await?;
    Ok(())
}

fn message(error: sea_orm::DbErr) -> loco_rs::Error {
    loco_rs::Error::Message(error.to_string())
}
