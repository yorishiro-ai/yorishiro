//! The queue-lifecycle protocol every target-restricted worker follows.
//!
//! A job that carries a `lifecycle_id` is admitted against `queue_job_lifecycles` before it runs, keeps its lease alive while it runs, and is recorded as completed or deferred afterwards.
//! A job without one (enqueued before lifecycle tracking existed) just runs.

use std::future::Future;

use loco_rs::{app::AppContext, bgworker::BackgroundWorker};
use serde::Serialize;
use uuid::Uuid;

use crate::models::queue_job_lifecycles::{Admission, Entity, LifecycleStatus};
use crate::workers::embedding_sync::WorkerClass;
use crate::workers::target::WorkerTarget;

const SATURATED: &str = "worker capacity saturated";

/// Records application lifecycle state without hiding handler failures from Loco.
/// Failed attempts remain eligible for Loco's explicit failed-job retry.
///
/// # Errors
/// Returns the handler error or a lifecycle persistence error.
pub(crate) async fn perform_with_terminal_lifecycle<W, A, F, Fut>(
    ctx: &AppContext,
    lifecycle_id: Option<Uuid>,
    class: WorkerClass,
    workspace_id: Uuid,
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
                        crate::time::now().fixed_offset(),
                    )
                    .await
                }
                None => Entity::defer(&ctx.db, id, None, SATURATED).await,
            }
            .map_err(message)?;
            return requeue::<W, A>(ctx, id, class, workspace_id, args).await;
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
            Err(error)
        }
    }
}

async fn requeue<W, A>(
    ctx: &AppContext,
    lifecycle_id: Uuid,
    class: WorkerClass,
    workspace_id: Uuid,
    args: &A,
) -> loco_rs::Result<()>
where
    W: BackgroundWorker<A>,
    A: Clone + Send + Sync + Serialize + 'static,
{
    let target = WorkerTarget::for_job(&ctx.db, class, workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let mut payload =
        serde_json::to_value(args).map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    if let serde_json::Value::Object(fields) = &mut payload {
        fields.insert(
            "lifecycle_id".into(),
            serde_json::Value::String(lifecycle_id.to_string()),
        );
        fields.insert(
            "workspace_id".into(),
            serde_json::Value::String(workspace_id.to_string()),
        );
    }
    let tenant_id =
        crate::models::workspace_workspaces::get_workspace_tenant(&ctx.db, workspace_id)
            .await
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let txn = crate::db::begin_workspace(ctx, tenant_id, workspace_id)
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    let tag = if super::registry::is_embedding_worker(&W::class_name()) {
        target.tag()
    } else {
        target.job_tag()
    };
    crate::models::queue_job_dispatch_outbox::record_lifecycle_payload(
        &txn,
        lifecycle_id,
        payload,
        W::class_name(),
        Some(tag.clone()),
        Some(vec![tag]),
    )
    .await
    .map_err(message)?;
    txn.commit()
        .await
        .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    Ok(())
}

fn message(error: sea_orm::DbErr) -> loco_rs::Error {
    loco_rs::Error::Message(error.to_string())
}
