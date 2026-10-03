use std::future::Future;

use async_trait::async_trait;
use loco_rs::{app::AppContext, bgworker::BackgroundWorker};
use uuid::Uuid;

use crate::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};
use crate::workers::dispatch::{EmbeddingSyncDispatcher, ReindexDispatcher};
use crate::workers::embedding_sync::{
    EmbeddingSyncArgs, EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared,
    EmbeddingSyncWorkerTenantPrivate, WorkerClass,
};
use crate::workers::reindex::{
    ReindexArgs, ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
};

use super::queue_concurrency_policy;

/// The single production adapter at Loco's worker integration point.
pub(crate) struct LocoJobDispatcher;

#[async_trait]
impl EmbeddingSyncDispatcher for LocoJobDispatcher {
    async fn dispatch(&self, ctx: &AppContext, args: EmbeddingSyncArgs) -> loco_rs::Result<String> {
        let spec = JobSpec {
            job_name: "embedding_sync",
            workspace_id: args.workspace_id,
            class: args.worker_class,
        };
        dispatch_job(ctx, spec, |lifecycle_id, priority| {
            let args = EmbeddingSyncArgs {
                lifecycle_id: Some(lifecycle_id),
                ..args
            };
            // Exhaustive, with no `_` arm: a new class without its worker type fails to compile instead of landing on the wrong tag.
            async move {
                match args.worker_class {
                    WorkerClass::TenantPrivate => {
                        EmbeddingSyncWorkerTenantPrivate::perform_later_with_priority(
                            ctx,
                            args,
                            Some(priority),
                        )
                        .await
                    }
                    WorkerClass::Official => {
                        EmbeddingSyncWorkerOfficial::perform_later_with_priority(
                            ctx,
                            args,
                            Some(priority),
                        )
                        .await
                    }
                    WorkerClass::Shared => {
                        EmbeddingSyncWorkerShared::perform_later_with_priority(
                            ctx,
                            args,
                            Some(priority),
                        )
                        .await
                    }
                }
            }
        })
        .await
    }
}

#[async_trait]
impl ReindexDispatcher for LocoJobDispatcher {
    async fn dispatch(&self, ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<String> {
        let spec = JobSpec {
            job_name: "reindex",
            workspace_id: args.workspace_id,
            class: args.worker_class,
        };
        dispatch_job(ctx, spec, |lifecycle_id, priority| {
            let args = ReindexArgs {
                lifecycle_id: Some(lifecycle_id),
                ..args
            };
            async move {
                match args.worker_class {
                    WorkerClass::TenantPrivate => {
                        ReindexWorkerTenantPrivate::perform_later_with_priority(
                            ctx,
                            args,
                            Some(priority),
                        )
                        .await
                    }
                    WorkerClass::Official => {
                        ReindexWorkerOfficial::perform_later_with_priority(
                            ctx,
                            args,
                            Some(priority),
                        )
                        .await
                    }
                    WorkerClass::Shared => {
                        ReindexWorkerShared::perform_later_with_priority(ctx, args, Some(priority))
                            .await
                    }
                }
            }
        })
        .await
    }
}

/// What identifies one job to the queue policy, independent of its argument type.
struct JobSpec {
    job_name: &'static str,
    workspace_id: Uuid,
    class: WorkerClass,
}

/// Admits one job: resolves its concurrency policy and priority, records the lifecycle row, then hands the job to the class's worker through `enqueue`.
///
/// Both policy lookups fail closed.
/// A job whose capacity or starvation policy cannot be determined is recorded as unavailable and refused, rather than enqueued under a guess that would let one class borrow another's reservation.
async fn dispatch_job<F, Fut>(
    ctx: &AppContext,
    spec: JobSpec,
    enqueue: F,
) -> loco_rs::Result<String>
where
    F: FnOnce(Uuid, i32) -> Fut,
    Fut: Future<Output = loco_rs::Result<String>>,
{
    let lifecycle_id = Uuid::now_v7();
    let class = spec.class.as_db_str();

    let (plan, concurrency_limit) = match queue_concurrency_policy(
        ctx,
        spec.workspace_id,
        spec.class,
    )
    .await
    {
        Ok(policy) => policy,
        Err(error) => {
            // No plan is known, so the row is closed with no capacity at all.
            refuse(ctx, &spec, lifecycle_id, None, class, 0, &error).await;
            tracing::error!(workspace_id = %spec.workspace_id, worker_class = class, diagnostic = %error, "queue policy unavailable");
            return Err(loco_rs::Error::Message(error));
        }
    };
    let concurrency_key = format!("{class}:{plan}");

    let scheduling = match crate::workers::queue::decide_for_dispatch(&ctx.db, spec.class).await {
        Ok(scheduling) => scheduling,
        Err(error) => {
            let diagnostic = format!("starvation policy lookup failed: {error}");
            refuse(
                ctx,
                &spec,
                lifecycle_id,
                Some(&plan),
                &concurrency_key,
                concurrency_limit,
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

    Entity::record_enqueue(
        &ctx.db,
        Enqueue {
            id: lifecycle_id,
            job_name: spec.job_name,
            worker_class: spec.class,
            workspace_id: Some(spec.workspace_id),
            plan: Some(&plan),
            concurrency_key: Some(&concurrency_key),
            concurrency_limit: Some(concurrency_limit),
        },
    )
    .await
    .map_err(|error| loco_rs::Error::Message(error.to_string()))?;

    match enqueue(lifecycle_id, scheduling.priority).await {
        Ok(job_id) => {
            if let Err(error) = Entity::mark_dispatched(&ctx.db, lifecycle_id, &job_id).await {
                tracing::error!(
                    lifecycle_id = %lifecycle_id,
                    provider_job_id = %job_id,
                    diagnostic = %error,
                    "provider job dispatched but lifecycle correlation write failed"
                );
            }
            Ok(job_id)
        }
        Err(error) => {
            close_unavailable(ctx, lifecycle_id, &error.to_string()).await;
            Err(error)
        }
    }
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
