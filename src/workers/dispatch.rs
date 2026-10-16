use async_trait::async_trait;
use loco_rs::{app::AppContext, bgworker::BackgroundWorker};
use uuid::Uuid;

use super::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorker, WorkerClass};
use super::reindex::{ReindexArgs, ReindexWorker};
use super::target::WorkerTarget;
use crate::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};

/// The typed dispatch seam used by the selected application enqueue paths.
#[async_trait]
pub trait EmbeddingSyncDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        ctx: &AppContext,
        args: EmbeddingSyncArgs,
        txn: Option<&sea_orm::DatabaseTransaction>,
    ) -> loco_rs::Result<String>;
}

/// The dispatch seam for reindex jobs.
///
/// `App::after_context` stores the production adapter as `Arc<dyn ReindexDispatcher>` in `AppContext::shared_store`.
/// A caller that needs a different dispatcher inserts its own `Arc<dyn ReindexDispatcher>` there afterwards.
#[async_trait]
pub trait ReindexDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        ctx: &AppContext,
        args: ReindexArgs,
        txn: Option<&sea_orm::DatabaseTransaction>,
    ) -> loco_rs::Result<String>;
}

/// The single production adapter at Loco's worker integration point.
pub(crate) struct LocoJobDispatcher;

#[async_trait]
impl EmbeddingSyncDispatcher for LocoJobDispatcher {
    async fn dispatch(
        &self,
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
}

#[async_trait]
impl ReindexDispatcher for LocoJobDispatcher {
    async fn dispatch(
        &self,
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
}

/// Re-enqueues durable dispatch records left queued after a process or provider failure.
pub async fn recover_pending(ctx: &AppContext) {
    let db_handle = ctx.shared_store.get::<crate::db::DbHandle>();
    let db = db_handle
        .as_ref()
        .map(crate::db::DbHandle::identity_connection);
    recover_pending_on(ctx, db.as_ref().unwrap_or(&ctx.db)).await;
}

pub(crate) fn start_relay(ctx: &AppContext) {
    if ctx.shared_store.get::<tokio::task::AbortHandle>().is_some() {
        return;
    }
    let relay_ctx = ctx.clone();
    let task = tokio::spawn(async move {
        loop {
            recover_pending(&relay_ctx).await;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });
    ctx.shared_store.insert(task.abort_handle());
}

pub async fn recover_pending_on(ctx: &AppContext, db: &sea_orm::DatabaseConnection) {
    let state = match crate::models::queue_job_dispatch_outbox::publishing_state(db).await {
        Ok(state) if state.active => state,
        Ok(_) => return,
        Err(error) => {
            tracing::warn!(error = %error, "dispatch outbox: publishing state unavailable");
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
    for row in pending {
        if row.publishing_epoch != state.epoch {
            continue;
        }
        let claim = Uuid::now_v7();
        match crate::models::queue_job_dispatch_outbox::claim_dispatch(
            db,
            row.id,
            state.epoch,
            claim,
            30,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                tracing::warn!(dispatch_id = %row.id, error = %error, "dispatch outbox: could not claim row");
                continue;
            }
        }
        if row.lifecycle_id.is_none()
            && row.worker_name
                != crate::workers::query_embedding::QueryEmbeddingWorker::class_name()
        {
            continue;
        }
        if let Some(deadline) = row.deadline_at
            && deadline <= chrono::Utc::now().fixed_offset()
        {
            if let Err(error) = crate::models::queue_job_dispatch_outbox::expire(db, row.id).await {
                tracing::warn!(dispatch_id = %row.id, error = %error, "could not expire deadline-bound dispatch");
            }
            continue;
        }
        let enqueue_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
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
                    state.epoch,
                    claim,
                    provider_job_id,
                )
                .await
                {
                    Ok(true) => {
                        // The acknowledgement transaction also records lifecycle correlation.
                    }
                    Ok(false) => {
                        tracing::warn!(dispatch_id = %row.id, "provider accepted dispatch after claim or epoch was fenced; duplicate risk retained")
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
    }
}

async fn enqueue_pending(
    ctx: &AppContext,
    row: &crate::models::queue_job_dispatch_outbox::PendingDispatch,
) -> loco_rs::Result<String> {
    let mut payload = row.payload.clone();
    if let serde_json::Value::Object(fields) = &mut payload {
        fields.insert(
            "dispatch_id".into(),
            serde_json::Value::String(row.id.to_string()),
        );
        fields.insert(
            "publishing_epoch".into(),
            serde_json::Value::from(row.publishing_epoch),
        );
        if let Some(workspace_id) = row.workspace_id {
            fields.insert(
                "workspace_id".into(),
                serde_json::Value::String(workspace_id.to_string()),
            );
        }
    }
    let queue = ctx
        .queue_provider
        .as_ref()
        .ok_or(loco_rs::Error::QueueProviderMissing)?;
    queue
        .enqueue(
            row.worker_name.clone(),
            row.queue_name.clone(),
            payload,
            row.tags.clone(),
            row.priority
                .or_else(|| Some(crate::workers::queue::decide(row.worker_class).priority)),
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
/// A job whose target, capacity or starvation policy cannot be determined is recorded as unavailable and refused, rather than enqueued under a guess that would let one class borrow another's reservation or send a tenant's work to the wrong workers.
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
            refuse(ctx, &spec, lifecycle_id, None, class, 0, &diagnostic).await;
            tracing::error!(workspace_id = %spec.workspace_id, worker_class = class, diagnostic = %diagnostic, "queue dispatch refused because the worker target is unknown");
            return Err(loco_rs::Error::Message(diagnostic));
        }
    };

    let (plan, concurrency_key, concurrency_limit) = if spec.use_queue_concurrency {
        let policy = match super::queue::concurrency_for(ctx, spec.workspace_id, spec.class).await {
            Ok(policy) => policy,
            Err(error) => {
                refuse(ctx, &spec, lifecycle_id, None, class, 0, &error).await;
                tracing::error!(workspace_id = %spec.workspace_id, worker_class = class, diagnostic = %error, "queue policy unavailable");
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

    let scheduling = match crate::workers::queue::decide_for_dispatch(&ctx.db, spec.class).await {
        Ok(scheduling) => scheduling,
        Err(error) => {
            let diagnostic = format!("starvation policy lookup failed: {error}");
            refuse(
                ctx,
                &spec,
                lifecycle_id,
                plan.as_deref(),
                concurrency_key.as_deref().unwrap_or(class),
                concurrency_limit.unwrap_or(0),
                &diagnostic,
            )
            .await;
            tracing::error!(
                lifecycle_id = %lifecycle_id,
                worker_class = class,
                policy = "starvation",
                degraded = true,
                diagnostic = %diagnostic,
                "queue dispatch refused because starvation policy is unavailable"
            );
            return Err(loco_rs::Error::Message(diagnostic));
        }
    };
    tracing::info!(
        lifecycle_id = %lifecycle_id,
        worker_class = class,
        scheduling_priority = scheduling.priority,
        fallback = scheduling.fallback,
        "queue scheduling decision"
    );

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
        Some(target.queue()),
        Some(vec![target.tag()]),
    )
    .await
    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    if let Some(owned_txn) = owned_txn {
        owned_txn
            .commit()
            .await
            .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
    }
    let _ = (scheduling, target);
    Ok(lifecycle_id.to_string())
}

/// Records a job that was refused before it reached the queue, so the refusal is visible next to the jobs that ran.
async fn refuse(
    ctx: &AppContext,
    spec: &JobSpec,
    lifecycle_id: Uuid,
    plan: Option<&str>,
    concurrency_key: &str,
    concurrency_limit: i32,
    diagnostic: &str,
) {
    if let Err(error) = Entity::record_enqueue(
        &ctx.db,
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
        tracing::warn!(lifecycle_id = %lifecycle_id, diagnostic = %error, "refused job could not be recorded");
        return;
    }
    close_unavailable(ctx, lifecycle_id, diagnostic).await;
}

async fn close_unavailable(ctx: &AppContext, lifecycle_id: Uuid, diagnostic: &str) {
    if let Err(error) = Entity::finish(
        &ctx.db,
        lifecycle_id,
        None,
        LifecycleStatus::Unavailable,
        Some(diagnostic),
    )
    .await
    {
        tracing::warn!(lifecycle_id = %lifecycle_id, diagnostic = %error, "unavailable job could not be closed");
    }
}
