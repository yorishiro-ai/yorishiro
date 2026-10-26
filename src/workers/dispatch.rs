use futures::{StreamExt, stream};
use loco_rs::{app::AppContext, bgworker::BackgroundWorker};
use uuid::Uuid;

use super::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorker, WorkerClass};
use super::reindex::{ReindexArgs, ReindexWorker};
use super::target::WorkerTarget;
use crate::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};

pub(crate) async fn dispatch_embedding(
    ctx: &AppContext,
    args: EmbeddingSyncArgs,
    txn: Option<&sea_orm::DatabaseTransaction>,
) -> loco_rs::Result<String> {
    let spec = JobSpec {
        job_name: EmbeddingSyncArgs::JOB_NAME,
        workspace_id: args.workspace_id,
        class: args.worker_class,
        dispatch_payload: serde_json::to_value(&args)
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?,
        worker_name: EmbeddingSyncWorker::class_name(),
        use_queue_concurrency: false,
    };
    dispatch_job(ctx, spec, txn).await
}

pub(crate) async fn dispatch_reindex(
    ctx: &AppContext,
    args: ReindexArgs,
    txn: Option<&sea_orm::DatabaseTransaction>,
) -> loco_rs::Result<String> {
    let spec = JobSpec {
        job_name: args.job_name(),
        workspace_id: args.workspace_id,
        class: args.worker_class,
        dispatch_payload: serde_json::to_value(&args)
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?,
        worker_name: ReindexWorker::class_name(),
        use_queue_concurrency: true,
    };
    dispatch_job(ctx, spec, txn).await
}

/// Re-enqueues durable dispatch records left queued after a process or provider failure.
pub async fn recover_pending(ctx: &AppContext) {
    recover_pending_while(ctx, || true).await;
}

pub(crate) async fn recover_pending_while(ctx: &AppContext, keep_running: impl Fn() -> bool) {
    let db_handle = ctx.shared_store.get::<crate::db::DbHandle>();
    let db = db_handle
        .as_ref()
        .map(crate::db::DbHandle::identity_connection);
    recover_pending_on_while(ctx, db.as_ref().unwrap_or(&ctx.db), keep_running).await;
}

async fn recover_pending_on_while(
    ctx: &AppContext,
    db: &sea_orm::DatabaseConnection,
    keep_running: impl Fn() -> bool,
) {
    let timing = match ctx.config.settings::<crate::data::settings::Settings>() {
        Ok(settings) => settings.dispatch,
        Err(error) => {
            tracing::error!(error = %error, "dispatch outbox: invalid application settings");
            return;
        }
    };
    let pending = match crate::models::queue_job_dispatch_outbox::due(db).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(error = %error, "dispatch outbox: recovery scan failed");
            return;
        }
    };
    // Bound provider pressure while letting unrelated deliveries pass a slow publish.
    let keep_running = &keep_running;
    stream::iter(pending).for_each_concurrent(4, |row| async move {
        if !keep_running() {
            return;
        }
        let claim = Uuid::now_v7();
        match crate::models::queue_job_dispatch_outbox::claim_dispatch(
            db,
            row.id,
            claim,
            timing.claim_lease_seconds.into(),
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                tracing::warn!(dispatch_id = %row.id, error = %error, "dispatch outbox: could not claim row");
                return;
            }
        }
        if row.lifecycle_id.is_none()
            && row.worker_name
                != crate::workers::query_embedding::QueryEmbeddingWorker::class_name()
        {
            return;
        }
        if let Some(deadline) = row.deadline_at
            && deadline <= crate::time::now().fixed_offset()
        {
            if let Err(error) = crate::models::queue_job_dispatch_outbox::expire(db, row.id).await {
                tracing::warn!(dispatch_id = %row.id, error = %error, "could not expire deadline-bound dispatch");
            }
            return;
        }
        let enqueue_result = tokio::time::timeout(
            std::time::Duration::from_millis(timing.enqueue_timeout_ms),
            enqueue_pending(ctx, &row),
        )
        .await;
        match &enqueue_result {
            Err(_) => {
                tracing::warn!(dispatch_id = %row.id, "publish timed out; provider outcome may be ambiguous")
            }
            Ok(Ok(provider_job_id)) => {
                match crate::models::queue_job_dispatch_outbox::acknowledge_dispatch(
                    db,
                    row.id,
                    claim,
                    provider_job_id,
                )
                .await
                {
                    Ok(true) => {
                        // The acknowledgement transaction also records lifecycle correlation.
                    }
                    Ok(false) => {
                        tracing::warn!(dispatch_id = %row.id, "provider accepted dispatch after its claim lease expired; duplicate risk retained")
                    }
                    Err(error) => {
                        tracing::warn!(dispatch_id = %row.id, error = %error, "provider accepted dispatch but acknowledgement failed")
                    }
                }
            }
            Ok(Err(error)) => tracing::warn!(
                dispatch_id = %row.id,
                job_name = %row.job_name,
                error = %error,
                "dispatch outbox: enqueue failed and will be retried"
            ),
        }
    }).await;
}

async fn enqueue_pending(
    ctx: &AppContext,
    row: &crate::models::queue_job_dispatch_outbox::PendingDispatch,
) -> loco_rs::Result<String> {
    let queue = ctx
        .queue_provider
        .as_ref()
        .ok_or(loco_rs::Error::QueueProviderMissing)?;
    queue
        .enqueue(
            row.worker_name.clone(),
            row.queue_name.clone(),
            row.payload.clone(),
            row.tags.clone(),
            row.priority
                .or_else(|| Some(crate::workers::queue::priority(row.worker_class))),
        )
        .await
        .map(|id| id.unwrap_or_else(|| Uuid::now_v7().to_string()))
        .map_err(|error| loco_rs::Error::Message(error.to_string()))
}

/// What identifies one job to the queue policy, independent of its argument type.
pub(crate) struct JobSpec {
    pub(crate) job_name: &'static str,
    pub(crate) workspace_id: Uuid,
    pub(crate) class: WorkerClass,
    pub(crate) dispatch_payload: serde_json::Value,
    pub(crate) worker_name: String,
    pub(crate) use_queue_concurrency: bool,
}

/// Admits one job: resolves its target, concurrency policy and priority, records the lifecycle row, then hands the job to the target's workers through `enqueue`.
///
/// Every lookup fails closed.
/// A job whose target or capacity policy cannot be determined is recorded as unavailable and refused, rather than enqueued under a guess that would let one class borrow another's reservation or send a tenant's work to the wrong workers.
pub(crate) async fn dispatch_job(
    ctx: &AppContext,
    spec: JobSpec,
    caller_txn: Option<&sea_orm::DatabaseTransaction>,
) -> loco_rs::Result<String> {
    let lifecycle_id = Uuid::now_v7();
    let class = spec.class.as_db_str();

    let target = match WorkerTarget::for_job(&ctx.db, spec.class, spec.workspace_id).await {
        Ok(target) => target,
        Err(error) => {
            let diagnostic = format!("worker target lookup failed: {error}");
            if let Some(txn) = caller_txn {
                refuse(txn, &spec, lifecycle_id, None, class, 0, &diagnostic).await;
            } else {
                refuse(&ctx.db, &spec, lifecycle_id, None, class, 0, &diagnostic).await;
            }
            tracing::error!(workspace_id = %spec.workspace_id, worker_class = class, error = %diagnostic, "queue dispatch refused because the worker target is unknown");
            return Err(loco_rs::Error::Message(diagnostic));
        }
    };

    let (plan, concurrency_key, concurrency_limit) = if spec.use_queue_concurrency {
        let policy = match super::queue::concurrency_for(ctx, spec.workspace_id, spec.class).await {
            Ok(policy) => policy,
            Err(error) => {
                if let Some(txn) = caller_txn {
                    refuse(txn, &spec, lifecycle_id, None, class, 0, &error).await;
                } else {
                    refuse(&ctx.db, &spec, lifecycle_id, None, class, 0, &error).await;
                }
                tracing::error!(workspace_id = %spec.workspace_id, worker_class = class, error = %error, "queue policy unavailable");
                return Err(loco_rs::Error::Message(error));
            }
        };
        (
            Some(policy.plan),
            Some(target.concurrency_key()),
            policy.limit,
        )
    } else {
        (None, None, None)
    };

    let payload = match spec.dispatch_payload {
        serde_json::Value::Object(mut fields) => {
            fields.insert(
                "lifecycle_id".into(),
                serde_json::Value::String(lifecycle_id.to_string()),
            );
            fields.insert(
                "workspace_id".into(),
                serde_json::Value::String(spec.workspace_id.to_string()),
            );
            serde_json::Value::Object(fields)
        }
        value => value,
    };
    let owned_txn = if caller_txn.is_none() {
        let tenant =
            crate::models::workspace_workspaces::get_workspace_tenant(&ctx.db, spec.workspace_id)
                .await
                .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        Some(
            crate::db::begin_workspace(ctx, tenant, spec.workspace_id)
                .await
                .map_err(|e| loco_rs::Error::Message(e.to_string()))?,
        )
    } else {
        None
    };
    let txn = caller_txn
        .or(owned_txn.as_ref())
        .expect("one transaction exists");
    let tag = if super::registry::is_embedding_worker(&spec.worker_name) {
        target.tag()
    } else {
        target.job_tag()
    };
    crate::models::queue_job_dispatch_outbox::record_in_transaction(
        txn,
        Enqueue {
            id: lifecycle_id,
            job_name: spec.job_name,
            worker_class: spec.class,
            workspace_id: Some(spec.workspace_id),
            plan: plan.as_deref(),
            concurrency_key: concurrency_key.as_deref(),
            concurrency_limit,
        },
        payload,
        spec.worker_name,
        Some(tag.clone()),
        Some(vec![tag]),
    )
    .await
    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    if let Some(owned_txn) = owned_txn {
        owned_txn
            .commit()
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
    }
    Ok(lifecycle_id.to_string())
}

/// Records a job that was refused before it reached the queue, so the refusal is visible next to the jobs that ran.
async fn refuse(
    db: &impl sea_orm::ConnectionTrait,
    spec: &JobSpec,
    lifecycle_id: Uuid,
    plan: Option<&str>,
    concurrency_key: &str,
    concurrency_limit: i32,
    diagnostic: &str,
) {
    if let Err(error) = Entity::record_enqueue(
        db,
        Enqueue {
            id: lifecycle_id,
            job_name: spec.job_name,
            worker_class: spec.class,
            workspace_id: Some(spec.workspace_id),
            plan,
            concurrency_key: Some(concurrency_key),
            concurrency_limit: Some(concurrency_limit),
        },
    )
    .await
    {
        tracing::warn!(lifecycle_id = %lifecycle_id, error = %error, "refused job could not be recorded");
        return;
    }
    close_unavailable(db, lifecycle_id, diagnostic).await;
}

async fn close_unavailable(
    db: &impl sea_orm::ConnectionTrait,
    lifecycle_id: Uuid,
    diagnostic: &str,
) {
    if let Err(error) = Entity::finish(
        db,
        lifecycle_id,
        None,
        LifecycleStatus::Unavailable,
        Some(diagnostic),
    )
    .await
    {
        tracing::warn!(lifecycle_id = %lifecycle_id, error = %error, "unavailable job could not be closed");
    }
}
