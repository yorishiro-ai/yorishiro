use std::future::Future;
#[cfg(feature = "test-support")]
use std::sync::Arc;

use async_trait::async_trait;
use loco_rs::{app::AppContext, bgworker::BackgroundWorker};
use uuid::Uuid;

use super::embedding_sync::{
    EmbeddingSyncArgs, EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared,
    EmbeddingSyncWorkerTenantPrivate, WorkerClass,
};
use super::reindex::{
    ReindexArgs, ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
};
use crate::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};

/// The typed dispatch seam used by the selected application enqueue paths.
#[async_trait]
pub(crate) trait EmbeddingSyncDispatcher: Send + Sync {
    async fn dispatch(&self, ctx: &AppContext, args: EmbeddingSyncArgs) -> loco_rs::Result<String>;
}

#[async_trait]
pub trait ReindexDispatcher: Send + Sync {
    async fn dispatch(&self, ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<String>;
}

/// The single production adapter at Loco's worker integration point.
pub(crate) struct LocoJobDispatcher;

#[async_trait]
impl EmbeddingSyncDispatcher for LocoJobDispatcher {
    async fn dispatch(&self, ctx: &AppContext, args: EmbeddingSyncArgs) -> loco_rs::Result<String> {
        let spec = JobSpec {
            job_name: EmbeddingSyncArgs::JOB_NAME,
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
            job_name: ReindexArgs::JOB_NAME,
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
pub(crate) struct JobSpec {
    pub(crate) job_name: &'static str,
    pub(crate) workspace_id: Uuid,
    pub(crate) class: WorkerClass,
}

/// Admits one job: resolves its concurrency policy and priority, records the lifecycle row, then hands the job to the class's worker through `enqueue`.
///
/// Both policy lookups fail closed.
/// A job whose capacity or starvation policy cannot be determined is recorded as unavailable and refused, rather than enqueued under a guess that would let one class borrow another's reservation.
pub(crate) async fn dispatch_job<F, Fut>(
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

    let (plan, concurrency_limit) = match super::queue::concurrency_for(
        ctx,
        spec.workspace_id,
        spec.class,
    )
    .await
    {
        Ok(policy) => (policy.plan, policy.limit),
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

#[cfg(feature = "test-support")]
/// Installs a fake reindex dispatcher for request-level tests.
pub fn install_test_reindex_dispatcher(ctx: &AppContext, dispatcher: Arc<dyn ReindexDispatcher>) {
    ctx.shared_store.insert(dispatcher);
}

#[cfg(test)]
pub(crate) async fn test_context() -> AppContext {
    use loco_rs::app::Hooks;
    use loco_rs::environment::Environment;
    use sea_orm::Database;

    let config = crate::app::App::load_config(&Environment::Any("test_sqlite".into()))
        .await
        .expect("load test configuration");
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect test database");
    AppContext::builder(Environment::Test, db, config).build()
}

/// What a provider swap must not change: a worker class keeps draining its own jobs however many another class has queued.
///
/// Redis filters tags client-side over the first 1000 entries of a queue, where the SQL providers filter in the query, so the production worker types have to give Redis a queue per class.
/// Skipped unless `YORISHIRO_REDIS_TEST_URL` names a Redis-compatible server.
#[cfg(test)]
mod redis_routing {
    use std::sync::Arc;
    use std::time::Duration;

    use loco_rs::bgworker::{self, BackgroundWorker};
    use loco_rs::config::RedisQueueConfig;
    use loco_rs::environment::Environment;
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{Database, EntityTrait};
    use tempfile::tempdir;

    use super::*;
    use crate::workers::embedding_sync::EmbeddingSyncWorkerOfficial;

    fn args(class: WorkerClass, lifecycle_id: Option<Uuid>) -> EmbeddingSyncArgs {
        EmbeddingSyncArgs {
            lifecycle_id,
            workspace_id: Uuid::nil(),
            entity_id: Uuid::nil(),
            worker_class: class,
        }
    }

    #[tokio::test]
    #[serial_test::serial(queue_postgres)]
    #[serial_test::serial(process_environment)]
    async fn a_class_drains_its_own_jobs_behind_a_backlog_of_another_class() {
        let Ok(uri) = std::env::var("YORISHIRO_REDIS_TEST_URL") else {
            eprintln!("skipping Redis routing test: YORISHIRO_REDIS_TEST_URL is unset");
            return;
        };
        if !uri.starts_with("redis://") && !uri.starts_with("rediss://") {
            eprintln!("skipping Redis routing test: explicit URL is not Redis");
            return;
        }
        if reqwest::Url::parse(&uri).map_or(true, |url| url.path() != "/15") {
            eprintln!("skipping Redis routing test: URL must select reserved test database 15");
            return;
        }

        let queue = Arc::new(
            bgworker::redis::create_provider(&RedisQueueConfig {
                uri,
                dangerously_flush: true,
                queues: Some(super::super::queue::class_queues()),
                num_workers: 1,
                reaper: None,
            })
            .await
            .expect("Redis queue"),
        );
        queue.clear().await.expect("clear Redis queue");

        let template = test_context().await;
        let path = tempdir().unwrap().keep().join("routing.sqlite3");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        Migrator::up(&db, None).await.unwrap();
        let ctx = AppContext::builder(Environment::Test, db, template.config.clone())
            .queue_provider(queue.clone())
            .build();

        queue
            .register(EmbeddingSyncWorkerShared::build(&ctx))
            .await
            .expect("register shared worker");

        // More than the 1000 entries Redis scans for a tag match.
        EmbeddingSyncWorkerOfficial::perform_all_later_with_priority(
            &ctx,
            (0..1100)
                .map(|_| (args(WorkerClass::Official, None), Some(200)))
                .collect(),
        )
        .await
        .expect("enqueue the official backlog");

        let id = Uuid::now_v7();
        Entity::record_enqueue(
            &ctx.db,
            Enqueue {
                id,
                job_name: "redis-routing-test",
                worker_class: WorkerClass::Shared,
                workspace_id: None,
                plan: None,
                concurrency_key: None,
                concurrency_limit: None,
            },
        )
        .await
        .unwrap();
        EmbeddingSyncWorkerShared::perform_later_with_priority(
            &ctx,
            args(WorkerClass::Shared, Some(id)),
            Some(100),
        )
        .await
        .expect("enqueue the shared job");

        let running = queue.clone();
        let handle = tokio::spawn(async move {
            running
                .run(vec![WorkerClass::Shared.tag().to_owned()])
                .await
        });
        let completed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let row = Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
                if row.status == LifecycleStatus::Completed.as_db_str() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        let _ = queue.shutdown();
        let _ = handle.await;
        let _ = queue.clear().await;

        assert!(
            completed.is_ok(),
            "the shared job was never run behind the official backlog"
        );
    }
}
