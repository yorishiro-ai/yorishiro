use loco_rs::app::AppContext;
use uuid::Uuid;
use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};
use yorishiro::workers::embedding_sync::{
    EmbeddingSyncArgs, EmbeddingSyncWorkerShared, WorkerClass,
};
/// What a provider swap must not change: a worker class keeps draining its own jobs however many another class has queued.
///
/// Redis filters tags client-side over the first 1000 entries of a queue, where the SQL providers filter in the query, so the production worker types have to give Redis a queue per class.
/// Skipped unless `YORISHIRO_REDIS_TEST_URL` names a Redis-compatible server.
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

    use yorishiro::workers::embedding_sync::EmbeddingSyncWorkerOfficial;

    /// This context is built by hand, so it carries none of the services `App::after_context` installs.
    /// The embedding worker treats a missing resolver as a wiring failure, so the test supplies the community rule: no workspace-specific provider.
    struct NoWorkspaceProvider;

    #[async_trait::async_trait]
    impl yorishiro::services::embedding::WorkspaceEmbeddingResolver for NoWorkspaceProvider {
        async fn resolve(
            &self,
            _conn: &sea_orm::DatabaseConnection,
            _workspace_id: Uuid,
        ) -> Result<
            Option<Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>,
            yorishiro::error::YorishiroError,
        > {
            Ok(None)
        }
    }

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
                queues: Some(yorishiro::workers::registry::WorkerRegistry::community().queues()),
                num_workers: 1,
                reaper: None,
            })
            .await
            .expect("Redis queue"),
        );
        queue.clear().await.expect("clear Redis queue");

        let template = crate::workers::test_context().await;
        let path = tempdir().unwrap().keep().join("routing.sqlite3");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        Migrator::up(&db, None).await.unwrap();
        let ctx = AppContext::builder(Environment::Test, db, template.config.clone())
            .queue_provider(queue.clone())
            .build();
        ctx.shared_store.insert(Arc::new(NoWorkspaceProvider)
            as Arc<dyn yorishiro::services::embedding::WorkspaceEmbeddingResolver>);

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
