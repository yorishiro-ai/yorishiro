use async_trait::async_trait;
use loco_rs::app::AppContext;

use crate::workers::dispatch::{EmbeddingSyncDispatcher, ReindexDispatcher};

use super::queue_concurrency_policy;

/// The single production adapter at Loco's worker integration point.
pub(crate) struct LocoJobDispatcher;

#[async_trait]
impl EmbeddingSyncDispatcher for LocoJobDispatcher {
    async fn dispatch(
        &self,
        ctx: &AppContext,
        mut args: crate::workers::embedding_sync::EmbeddingSyncArgs,
    ) -> loco_rs::Result<String> {
        use crate::workers::embedding_sync::{
            EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared,
            EmbeddingSyncWorkerTenantPrivate, WorkerClass,
        };
        use loco_rs::bgworker::BackgroundWorker;

        let lifecycle_id = uuid::Uuid::now_v7();
        let worker_class = args.worker_class.as_db_str();
        let (plan, concurrency_limit) = match queue_concurrency_policy(
            ctx,
            args.workspace_id,
            worker_class,
        )
        .await
        {
            Ok(policy) => policy,
            Err(error) => {
                let _ = crate::models::queue_job_lifecycles::Entity::record_enqueue(
                    &ctx.db,
                    crate::models::queue_job_lifecycles::Enqueue {
                        id: lifecycle_id,
                        job_name: "embedding_sync",
                        worker_class,
                        workspace_id: Some(args.workspace_id),
                        plan: None,
                        concurrency_key: Some(worker_class),
                        concurrency_limit: Some(0),
                    },
                )
                .await;
                let _ = crate::models::queue_job_lifecycles::Entity::finish(
                    &ctx.db,
                    lifecycle_id,
                    None,
                    "unavailable",
                    Some(&error),
                )
                .await;
                tracing::error!(workspace_id = %args.workspace_id, worker_class, diagnostic = %error, "queue policy unavailable");
                return Err(loco_rs::Error::Message(error));
            }
        };
        let concurrency_key = format!("{worker_class}:{plan}");
        let scheduling =
            match crate::services::queue::decide_for_dispatch(&ctx.db, args.worker_class).await {
                Ok(scheduling) => scheduling,
                Err(error) => {
                    let diagnostic = format!("starvation policy lookup failed: {error}");
                    let _ = crate::models::queue_job_lifecycles::Entity::record_enqueue(
                        &ctx.db,
                        crate::models::queue_job_lifecycles::Enqueue {
                            id: lifecycle_id,
                            job_name: "embedding_sync",
                            worker_class,
                            workspace_id: Some(args.workspace_id),
                            plan: Some(&plan),
                            concurrency_key: Some(&concurrency_key),
                            concurrency_limit: Some(concurrency_limit),
                        },
                    )
                    .await;
                    let _ = crate::models::queue_job_lifecycles::Entity::finish(
                        &ctx.db,
                        lifecycle_id,
                        None,
                        "unavailable",
                        Some(&diagnostic),
                    )
                    .await;
                    tracing::error!(
                        lifecycle_id = %lifecycle_id,
                        worker_class,
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
            worker_class,
            scheduling_priority = scheduling.priority,
            fallback = scheduling.fallback,
            "queue scheduling decision"
        );
        crate::models::queue_job_lifecycles::Entity::record_enqueue(
            &ctx.db,
            crate::models::queue_job_lifecycles::Enqueue {
                id: lifecycle_id,
                job_name: "embedding_sync",
                worker_class,
                workspace_id: Some(args.workspace_id),
                plan: Some(&plan),
                concurrency_key: Some(&concurrency_key),
                concurrency_limit: Some(concurrency_limit),
            },
        )
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        args.lifecycle_id = Some(lifecycle_id);
        let result = match args.worker_class {
            WorkerClass::TenantPrivate => {
                EmbeddingSyncWorkerTenantPrivate::perform_later_with_priority(
                    ctx,
                    args,
                    Some(scheduling.priority),
                )
                .await
            }
            WorkerClass::Official => {
                EmbeddingSyncWorkerOfficial::perform_later_with_priority(
                    ctx,
                    args,
                    Some(scheduling.priority),
                )
                .await
            }
            WorkerClass::Shared => {
                EmbeddingSyncWorkerShared::perform_later_with_priority(
                    ctx,
                    args,
                    Some(scheduling.priority),
                )
                .await
            }
        };
        match result {
            Ok(job_id) => {
                if let Err(error) = crate::models::queue_job_lifecycles::Entity::mark_dispatched(
                    &ctx.db,
                    lifecycle_id,
                    &job_id,
                )
                .await
                {
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
                let _ = crate::models::queue_job_lifecycles::Entity::finish(
                    &ctx.db,
                    lifecycle_id,
                    None,
                    "unavailable",
                    Some(&error.to_string()),
                )
                .await;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl ReindexDispatcher for LocoJobDispatcher {
    async fn dispatch(
        &self,
        ctx: &AppContext,
        mut args: crate::workers::reindex::ReindexArgs,
    ) -> loco_rs::Result<String> {
        use crate::workers::embedding_sync::WorkerClass;
        use crate::workers::reindex::{
            ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
        };
        use loco_rs::bgworker::BackgroundWorker;

        let lifecycle_id = uuid::Uuid::now_v7();
        let worker_class = args.worker_class.as_db_str();
        let (plan, concurrency_limit) = match queue_concurrency_policy(
            ctx,
            args.workspace_id,
            worker_class,
        )
        .await
        {
            Ok(policy) => policy,
            Err(error) => {
                let _ = crate::models::queue_job_lifecycles::Entity::record_enqueue(
                    &ctx.db,
                    crate::models::queue_job_lifecycles::Enqueue {
                        id: lifecycle_id,
                        job_name: "reindex",
                        worker_class,
                        workspace_id: Some(args.workspace_id),
                        plan: None,
                        concurrency_key: Some(worker_class),
                        concurrency_limit: Some(0),
                    },
                )
                .await;
                let _ = crate::models::queue_job_lifecycles::Entity::finish(
                    &ctx.db,
                    lifecycle_id,
                    None,
                    "unavailable",
                    Some(&error),
                )
                .await;
                tracing::error!(workspace_id = %args.workspace_id, worker_class, diagnostic = %error, "queue policy unavailable");
                return Err(loco_rs::Error::Message(error));
            }
        };
        let concurrency_key = format!("{worker_class}:{plan}");
        let scheduling = crate::services::queue::decide_for_dispatch(&ctx.db, args.worker_class)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(error = %error, "queue starvation lookup failed");
                crate::services::queue::decide(args.worker_class)
            });
        tracing::info!(
            lifecycle_id = %lifecycle_id,
            worker_class,
            scheduling_priority = scheduling.priority,
            fallback = scheduling.fallback,
            "queue scheduling decision"
        );
        crate::models::queue_job_lifecycles::Entity::record_enqueue(
            &ctx.db,
            crate::models::queue_job_lifecycles::Enqueue {
                id: lifecycle_id,
                job_name: "reindex",
                worker_class,
                workspace_id: Some(args.workspace_id),
                plan: Some(&plan),
                concurrency_key: Some(&concurrency_key),
                concurrency_limit: Some(concurrency_limit),
            },
        )
        .await
        .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
        args.lifecycle_id = Some(lifecycle_id);
        let result = match args.worker_class {
            WorkerClass::TenantPrivate => {
                ReindexWorkerTenantPrivate::perform_later_with_priority(
                    ctx,
                    args,
                    Some(scheduling.priority),
                )
                .await
            }
            WorkerClass::Official => {
                ReindexWorkerOfficial::perform_later_with_priority(
                    ctx,
                    args,
                    Some(scheduling.priority),
                )
                .await
            }
            WorkerClass::Shared => {
                ReindexWorkerShared::perform_later_with_priority(
                    ctx,
                    args,
                    Some(scheduling.priority),
                )
                .await
            }
        };
        match result {
            Ok(job_id) => {
                if let Err(error) = crate::models::queue_job_lifecycles::Entity::mark_dispatched(
                    &ctx.db,
                    lifecycle_id,
                    &job_id,
                )
                .await
                {
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
                let _ = crate::models::queue_job_lifecycles::Entity::finish(
                    &ctx.db,
                    lifecycle_id,
                    None,
                    "unavailable",
                    Some(&error.to_string()),
                )
                .await;
                Err(error)
            }
        }
    }
}
