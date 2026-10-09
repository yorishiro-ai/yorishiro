/// Tests for the embedding sync worker: worker class tags, serialization, and class names.
use loco_rs::bgworker::BackgroundWorker;
use yorishiro::workers::embedding_sync::{
    EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared, EmbeddingSyncWorkerTenantPrivate,
    WorkerClass,
};

/// Each of the three worker types must carry exactly its own `WorkerClass`'s tag and no other: a type whose `tags()` drifted to list a second class's tag (or dropped its own) would let a tag-restricted worker process either miss its own jobs or pick up another class's, exactly the bug this whole split exists to close.
#[test]
fn each_worker_type_carries_exactly_its_own_class_tag() {
    assert_eq!(
        EmbeddingSyncWorkerTenantPrivate::tags(),
        vec!["worker-class:tenant-private".to_string()]
    );
    assert_eq!(
        EmbeddingSyncWorkerOfficial::tags(),
        vec!["worker-class:official".to_string()]
    );
    assert_eq!(
        EmbeddingSyncWorkerShared::tags(),
        vec!["worker-class:shared".to_string()]
    );
}

/// `serde(rename_all = "snake_case")` is what `EmbeddingSyncArgs` persists in the queue payload; asserting the wire form catches an accidental rename breaking a job already sitting in a queue at deploy time.
#[test]
fn worker_class_serializes_to_snake_case() {
    assert_eq!(
        serde_json::to_value(WorkerClass::TenantPrivate).unwrap(),
        serde_json::json!("tenant_private")
    );
    assert_eq!(
        serde_json::to_value(WorkerClass::Official).unwrap(),
        serde_json::json!("official")
    );
    assert_eq!(
        serde_json::to_value(WorkerClass::Shared).unwrap(),
        serde_json::json!("shared")
    );
}

/// `as_db_str`/`from_db_str` must round-trip every variant, and must agree with the `snake_case` serde wire form above: `ee/`'s `workspace_worker_classes` stores this same string, so a row read from the database and a value read off a queued job's payload must be indistinguishable.
#[test]
fn db_str_round_trips_and_matches_the_serde_wire_form() {
    for class in [
        WorkerClass::TenantPrivate,
        WorkerClass::Official,
        WorkerClass::Shared,
    ] {
        let db_str = class.as_db_str();
        assert_eq!(
            WorkerClass::from_db_str(db_str).unwrap(),
            class,
            "as_db_str/from_db_str must round-trip {class:?}"
        );
        assert_eq!(
            serde_json::to_value(class).unwrap(),
            serde_json::json!(db_str),
            "{class:?}'s db string must match its serde wire form"
        );
    }
}

#[test]
fn from_db_str_rejects_an_unknown_value() {
    assert!(WorkerClass::from_db_str("not-a-real-class").is_none());
}

/// `App::connect_workers` registers each of the three types under its own `class_name()`.
/// Registering needs a real `Queue`, which a unit test has no access to, so this guards the
/// assumption instead: three distinct class names, or one `register` call silently clobbers
/// another's handler rather than adding a third.
/// `enqueue_for_class`'s exhaustive `match` on `WorkerClass` already forces a compile error if a fourth variant is added with no worker type to dispatch to; this test covers the complementary runtime gap `connect_workers` itself has no compiler check for: a worker type that exists and is dispatched to, but was never actually registered.
#[test]
fn the_three_worker_types_have_distinct_class_names() {
    let names = [
        EmbeddingSyncWorkerTenantPrivate::class_name(),
        EmbeddingSyncWorkerOfficial::class_name(),
        EmbeddingSyncWorkerShared::class_name(),
    ];
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "worker types must have distinct class_name()s, got {names:?}"
    );
}

mod unit {
    use async_trait::async_trait;
    use loco_rs::app::AppContext;

    use std::sync::Mutex;
    use uuid::Uuid;

    use yorishiro::workers::dispatch::EmbeddingSyncDispatcher;

    use yorishiro::workers::embedding_sync::*;

    struct RecordingDispatcher {
        args: Mutex<Vec<EmbeddingSyncArgs>>,
        fail: bool,
    }

    #[async_trait]
    impl EmbeddingSyncDispatcher for RecordingDispatcher {
        async fn dispatch(
            &self,
            _ctx: &AppContext,
            args: EmbeddingSyncArgs,
        ) -> loco_rs::Result<String> {
            self.args.lock().unwrap().push(args);
            if self.fail {
                Err(loco_rs::Error::Message("dispatch failed".into()))
            } else {
                Ok("embedding-job".into())
            }
        }
    }

    #[tokio::test]
    async fn dispatches_the_original_args_and_returns_success() {
        let ctx = crate::workers::test_context().await;
        let args = EmbeddingSyncArgs {
            lifecycle_id: None,
            workspace_id: Uuid::now_v7(),
            entity_id: Uuid::now_v7(),
            worker_class: WorkerClass::Official,
        };
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: false,
        };

        enqueue_for_class_with_dispatcher(&ctx, args.clone(), &dispatcher)
            .await
            .expect("dispatch");

        let recorded = dispatcher.args.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].workspace_id, args.workspace_id);
        assert_eq!(recorded[0].entity_id, args.entity_id);
        assert_eq!(recorded[0].worker_class, args.worker_class);
    }

    #[tokio::test]
    async fn preserves_dispatch_failure() {
        let ctx = crate::workers::test_context().await;
        let dispatcher = RecordingDispatcher {
            args: Mutex::new(Vec::new()),
            fail: true,
        };

        let error = enqueue_for_class_with_dispatcher(
            &ctx,
            EmbeddingSyncArgs {
                lifecycle_id: None,
                workspace_id: Uuid::now_v7(),
                entity_id: Uuid::now_v7(),
                worker_class: WorkerClass::Shared,
            },
            &dispatcher,
        )
        .await
        .expect_err("dispatch must fail");

        assert_eq!(error.to_string(), "dispatch failed");
    }
}

mod conflict {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use loco_rs::app::AppContext;
    use loco_rs::bgworker::{self, BackgroundWorker, sqlt};
    use loco_rs::config::SqliteQueueConfig;
    use sea_orm::{
        ActiveModelTrait, DatabaseConnection, EntityTrait, FromQueryResult, IntoActiveModel,
        Statement,
    };
    use serial_test::serial;
    use tokio::sync::{Notify, Semaphore};
    use uuid::Uuid;
    use yorishiro::App;
    use yorishiro::error::YorishiroError;
    use yorishiro::models::_entities::{api_keys, tenant_tenants, workspace_workspaces};
    use yorishiro::models::api_keys::ApiKeyScope;
    use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};
    use yorishiro::models::tenant_memberships::MembershipRole;
    use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
    use yorishiro::models::{entity_entities, schema_schemas};
    use yorishiro::services::embedding::{
        EmbedKind, EmbeddingProvider, WorkspaceEmbeddingResolver,
    };
    use yorishiro::workers::embedding_sync::{
        EmbeddingSyncArgs, EmbeddingSyncWorkerShared, WorkerClass,
    };

    use crate::requests::boot_request;

    /// Updates the entity once, during the first document embedding.
    /// The worker has already read its snapshot by then, so the snapshot's token is stale when the vector is persisted.
    struct MutatingProvider {
        db: DatabaseConnection,
        workspace_id: Uuid,
        entity_id: Uuid,
        delete: bool,
        mutate: AtomicBool,
        width: usize,
    }

    enum FailureKind {
        Busy,
        Unreachable,
    }

    struct FailingProvider {
        kind: FailureKind,
    }

    struct DisabledProvider;

    struct GatedProvider {
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        entered: Notify,
        release: Semaphore,
        width: usize,
    }

    struct TestResolver(Arc<dyn EmbeddingProvider>);

    #[async_trait]
    impl WorkspaceEmbeddingResolver for TestResolver {
        async fn resolve(
            &self,
            _db: &DatabaseConnection,
            _workspace_id: Uuid,
        ) -> Result<Option<Arc<dyn EmbeddingProvider>>, YorishiroError> {
            Ok(Some(self.0.clone()))
        }
    }

    #[async_trait]
    impl EmbeddingProvider for GatedProvider {
        fn dimensions(&self) -> usize {
            self.width
        }
        fn model_name(&self) -> String {
            "gated-test-provider".into()
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            self.entered.notify_waiters();
            self.release.acquire().await.unwrap().forget();
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(texts.iter().map(|_| vec![1.0; self.width]).collect())
        }
    }

    #[async_trait]
    impl EmbeddingProvider for DisabledProvider {
        fn availability(&self) -> yorishiro::services::embedding::EmbeddingProviderAvailability {
            yorishiro::services::embedding::EmbeddingProviderAvailability::Disabled
        }

        fn dimensions(&self) -> usize {
            768
        }

        fn model_name(&self) -> String {
            "disabled-test-provider".into()
        }

        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            panic!("disabled providers must not be called")
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FailingProvider {
        fn dimensions(&self) -> usize {
            768
        }

        fn model_name(&self) -> String {
            "failing-test-provider".into()
        }

        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            Err(match self.kind {
                FailureKind::Busy => YorishiroError::ProviderBusy {
                    message: "test provider is busy".into(),
                    retry_after: std::time::Duration::from_secs(1),
                },
                FailureKind::Unreachable => YorishiroError::ProviderUnreachable {
                    url: "http://test-provider".into(),
                    message: "test provider is unreachable".into(),
                },
            })
        }
    }

    #[async_trait]
    impl EmbeddingProvider for MutatingProvider {
        fn dimensions(&self) -> usize {
            self.width
        }

        fn model_name(&self) -> String {
            "mutating-test-provider".into()
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            Ok(texts.iter().map(|_| vec![1.0; self.width]).collect())
        }

        async fn embed_as(&self, kind: EmbedKind, _text: &str) -> Result<Vec<f32>, YorishiroError> {
            if kind == EmbedKind::Document && self.mutate.swap(false, Ordering::SeqCst) {
                if self.delete {
                    entity_entities::delete(&self.db, self.workspace_id, self.entity_id).await?;
                    return Ok(vec![1.0; self.width]);
                }
                entity_entities::update(
                    &self.db,
                    self.workspace_id,
                    entity_entities::UpdateEntityInput {
                        id: self.entity_id,
                        data: serde_json::json!({ "title": "changed while embedding" }),
                        updated_by: None,
                    },
                )
                .await?;
            }
            Ok(vec![1.0; self.width])
        }
    }

    async fn seed(ctx: &AppContext) -> (Uuid, Uuid) {
        let tenant = tenant_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("worker-conflict".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert tenant");
        let workspace = workspace_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert workspace");
        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": { "note": { "fields": {
                "title": { "type": "string", "required": true, "x-embed": true }
            } } }
        }))
        .expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant.id, workspace.id, definition, None, None)
            .await
            .expect("create schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace.id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "original" }),
            },
            None,
        )
        .await
        .expect("create entity");
        (workspace.id, entity.id)
    }

    async fn seed_without_embeddable_content(ctx: &AppContext) -> (Uuid, Uuid) {
        let tenant = tenant_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("worker-no-content".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert tenant");
        let workspace = workspace_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert workspace");
        let definition = serde_json::from_value(serde_json::json!({
            "name": "plain",
            "entity_types": { "plain": { "fields": {
                "title": { "type": "string", "required": true }
            } } }
        }))
        .expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant.id, workspace.id, definition, None, None)
            .await
            .expect("create schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace.id,
            entity_entities::CreateEntityInput {
                schema_name: "plain".into(),
                entity_type: "plain".into(),
                data: serde_json::json!({ "title": "not embedded" }),
            },
            None,
        )
        .await
        .expect("create entity");
        (workspace.id, entity.id)
    }

    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }

    async fn vector_rows(ctx: &AppContext, entity_id: Uuid) -> i64 {
        Count::find_by_statement(Statement::from_sql_and_values(
            sea_orm::ConnectionTrait::get_database_backend(&ctx.db),
            "SELECT COUNT(*) AS n FROM entity_embeddings_768 WHERE entity_id = $1",
            [entity_id.into()],
        ))
        .one(&ctx.db)
        .await
        .expect("count vectors")
        .expect("count row")
        .n
    }

    #[tokio::test]
    async fn provider_failures_are_terminal_and_do_not_requeue() {
        boot_request::<App, _, _>(|_request, ctx| async move {
            let (workspace_id, entity_id) = seed(&ctx).await;
            let directory = tempfile::tempdir().expect("queue tempdir");
            let queue_uri = format!(
                "sqlite://{}?mode=rwc",
                directory.path().join("queue.sqlite3").display()
            );
            let queue = bgworker::sqlt::create_provider(&SqliteQueueConfig {
                uri: queue_uri.clone(),
                dangerously_flush: false,
                enable_logging: false,
                max_connections: 2,
                min_connections: 1,
                connect_timeout: 5_000,
                idle_timeout: 5_000,
                poll_interval_sec: 1,
                num_workers: 1,
                reaper: None,
            })
            .await
            .expect("SQLite queue");
            queue.setup().await.expect("set up queue");
            let queue = Arc::new(queue);
            let ctx = ctx.into_builder().queue_provider(queue.clone()).build();
            let queue_pool = sqlx::SqlitePool::connect(&queue_uri)
                .await
                .expect("connect to the queue file");

            for (lifecycle_id, kind) in [
                (Uuid::now_v7(), FailureKind::Busy),
                (Uuid::now_v7(), FailureKind::Unreachable),
            ] {
                ctx.shared_store
                    .insert(Arc::new(FailingProvider { kind }) as Arc<dyn EmbeddingProvider>);
                Entity::record_enqueue(
                    &ctx.db,
                    Enqueue {
                        id: lifecycle_id,
                        job_name: "embedding_sync",
                        worker_class: WorkerClass::Shared,
                        workspace_id: Some(workspace_id),
                        plan: None,
                        concurrency_key: None,
                        concurrency_limit: None,
                    },
                )
                .await
                .expect("record lifecycle");

                let result = EmbeddingSyncWorkerShared::build(&ctx)
                    .perform(EmbeddingSyncArgs {
                        lifecycle_id: Some(lifecycle_id),
                        workspace_id,
                        entity_id,
                        worker_class: WorkerClass::Shared,
                    })
                    .await;
                assert!(result.is_err(), "provider failure must reach Loco");
                let lifecycle = Entity::find_by_id(lifecycle_id)
                    .one(&ctx.db)
                    .await
                    .expect("read lifecycle")
                    .expect("lifecycle row");
                assert_eq!(lifecycle.status, LifecycleStatus::Failed.as_db_str());
                assert!(lifecycle.error.is_some());
                assert!(
                    sqlt::get_jobs(&queue_pool, None, None)
                        .await
                        .expect("read queue")
                        .is_empty()
                );
            }

            // Also drive one failure through Loco's provider runner so the queue's own job
            // status is observed alongside the custom lifecycle status.
            let live_id = Uuid::now_v7();
            ctx.shared_store.insert(Arc::new(FailingProvider {
                kind: FailureKind::Busy,
            }) as Arc<dyn EmbeddingProvider>);
            Entity::record_enqueue(
                &ctx.db,
                Enqueue {
                    id: live_id,
                    job_name: "embedding_sync",
                    worker_class: WorkerClass::Shared,
                    workspace_id: Some(workspace_id),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .expect("record live failure lifecycle");
            EmbeddingSyncWorkerShared::perform_later_with_priority(
                &ctx,
                EmbeddingSyncArgs {
                    lifecycle_id: Some(live_id),
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                },
                Some(100),
            )
            .await
            .expect("enqueue live failure");
            queue
                .register(EmbeddingSyncWorkerShared::build(&ctx))
                .await
                .expect("register embedding worker");
            let running = queue.clone();
            let worker =
                tokio::spawn(
                    async move { running.run(vec![WorkerClass::Shared.tag().into()]).await },
                );
            let failed_jobs = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let jobs = sqlt::get_jobs(
                        &queue_pool,
                        Some(&vec![loco_rs::bgworker::JobStatus::Failed]),
                        None,
                    )
                    .await
                    .expect("read failed jobs");
                    if !jobs.is_empty() {
                        break jobs;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("live queue job must fail");
            assert_eq!(failed_jobs.len(), 1);
            let live_lifecycle = Entity::find_by_id(live_id)
                .one(&ctx.db)
                .await
                .expect("read live lifecycle")
                .expect("live lifecycle row");
            assert_eq!(live_lifecycle.status, LifecycleStatus::Failed.as_db_str());
            assert_eq!(
                sqlt::get_jobs(&queue_pool, None, None)
                    .await
                    .expect("read final queue")
                    .len(),
                failed_jobs.len()
            );
            let _ = queue.shutdown();
            worker
                .await
                .expect("join failure worker")
                .expect("run failure worker");

            let _ = queue.shutdown();
            queue_pool.close().await;
        })
        .await;
    }

    #[tokio::test]
    async fn three_document_worker_entries_share_the_installed_limiter() {
        let _guard = crate::EnvGuard::capture(&["DATABASE_URL", "QUEUE_URL"]);
        _guard.set(
            "DATABASE_URL",
            "sqlite:///tmp/embedding-concurrency.sqlite3?mode=rwc",
        );
        _guard.set(
            "QUEUE_URL",
            "sqlite:///tmp/embedding-concurrency-queue.sqlite3?mode=rwc",
        );
        boot_request::<App, _, _>(|_request, ctx| async move {
            let (workspace_id, first) = seed(&ctx).await;
            let second = entity_entities::create(
                &ctx.db,
                workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: serde_json::json!({"title":"second"}),
                },
                None,
            )
            .await
            .unwrap()
            .id;
            let third = entity_entities::create(
                &ctx.db,
                workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: serde_json::json!({"title":"third"}),
                },
                None,
            )
            .await
            .unwrap()
            .id;
            let provider = Arc::new(GatedProvider {
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                entered: Notify::new(),
                release: Semaphore::new(0),
                width: 768,
            });
            ctx.shared_store
                .insert(provider.clone() as Arc<dyn EmbeddingProvider>);
            ctx.shared_store.insert(
                Arc::new(TestResolver(provider.clone())) as Arc<dyn WorkspaceEmbeddingResolver>
            );
            let directory = tempfile::tempdir().expect("queue tempdir");
            let queue_uri = format!(
                "sqlite://{}?mode=rwc",
                directory.path().join("queue.sqlite3").display()
            );
            let queue = Arc::new(
                bgworker::sqlt::create_provider(&SqliteQueueConfig {
                    uri: queue_uri.clone(),
                    dangerously_flush: false,
                    enable_logging: false,
                    max_connections: 2,
                    min_connections: 1,
                    connect_timeout: 5_000,
                    idle_timeout: 5_000,
                    poll_interval_sec: 1,
                    num_workers: 3,
                    reaper: None,
                })
                .await
                .expect("SQLite queue"),
            );
            queue.setup().await.expect("set up queue");
            let queue_pool = sqlx::SqlitePool::connect(&queue_uri)
                .await
                .expect("queue pool");
            let ctx = ctx.into_builder().queue_provider(queue.clone()).build();
            queue
                .register(EmbeddingSyncWorkerShared::build(&ctx))
                .await
                .expect("register worker");
            let ids = [first, second, third];
            let mut lifecycle_ids = Vec::new();
            for entity_id in ids {
                let lifecycle_id = Uuid::now_v7();
                lifecycle_ids.push(lifecycle_id);
                Entity::record_enqueue(
                    &ctx.db,
                    Enqueue {
                        id: lifecycle_id,
                        job_name: "embedding_sync",
                        worker_class: WorkerClass::Shared,
                        workspace_id: Some(workspace_id),
                        plan: None,
                        concurrency_key: None,
                        concurrency_limit: None,
                    },
                )
                .await
                .unwrap();
                queue
                    .enqueue(
                        EmbeddingSyncWorkerShared::class_name(),
                        None,
                        serde_json::to_value(EmbeddingSyncArgs {
                            lifecycle_id: Some(lifecycle_id),
                            workspace_id,
                            entity_id,
                            worker_class: WorkerClass::Shared,
                        })
                        .unwrap(),
                        Some(vec![WorkerClass::Shared.tag().into()]),
                        Some(100),
                    )
                    .await
                    .expect("enqueue document job");
            }
            let initial_jobs = sqlt::get_jobs(&queue_pool, None, None)
                .await
                .expect("count jobs")
                .len();
            assert_eq!(initial_jobs, 3);
            let running = queue.clone();
            let runner =
                tokio::spawn(
                    async move { running.run(vec![WorkerClass::Shared.tag().into()]).await },
                );
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while provider.calls.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("first provider call entered");
            tokio::task::yield_now().await;
            assert_eq!(provider.active.load(Ordering::SeqCst), 1);
            assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
            let rows = Entity::find().all(&ctx.db).await.expect("read lifecycles");
            assert_eq!(
                rows.iter()
                    .filter(|row| lifecycle_ids.contains(&row.id)
                        && row.status == LifecycleStatus::Running.as_db_str())
                    .count(),
                1
            );
            assert_eq!(
                rows.iter()
                    .filter(|row| lifecycle_ids.contains(&row.id)
                        && row.status == LifecycleStatus::Queued.as_db_str())
                    .count(),
                2
            );
            assert_eq!(
                rows.iter()
                    .filter(|row| lifecycle_ids.contains(&row.id)
                        && row.status == LifecycleStatus::Failed.as_db_str())
                    .count(),
                0
            );
            assert_eq!(
                sqlt::get_jobs(&queue_pool, None, None)
                    .await
                    .expect("count blocked jobs")
                    .len(),
                initial_jobs
            );
            let processing = sqlt::get_jobs(
                &queue_pool,
                Some(&vec![loco_rs::bgworker::JobStatus::Processing]),
                None,
            )
            .await
            .expect("read dispatched jobs");
            assert_eq!(
                processing.len(),
                3,
                "all consumers must have dequeued a job"
            );
            provider.release.add_permits(3);
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let done = Entity::find()
                        .all(&ctx.db)
                        .await
                        .unwrap()
                        .into_iter()
                        .filter(|row| {
                            lifecycle_ids.contains(&row.id)
                                && row.status == LifecycleStatus::Completed.as_db_str()
                        })
                        .count();
                    if done == 3 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("all queued jobs complete");
            let _ = queue.shutdown();
            runner.await.unwrap().unwrap();
            assert_eq!(
                sqlt::get_jobs(&queue_pool, None, None)
                    .await
                    .expect("final queue")
                    .len(),
                3
            );
            assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
            assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
            assert_eq!(vector_rows(&ctx, first).await, 1);
            assert_eq!(vector_rows(&ctx, second).await, 1);
            assert_eq!(vector_rows(&ctx, third).await, 1);
        })
        .await;
    }

    #[tokio::test]
    async fn no_content_and_disabled_provider_are_successful_noops() {
        boot_request::<App, _, _>(|_request, ctx| async move {
            let (workspace_id, entity_id) = seed_without_embeddable_content(&ctx).await;
            ctx.shared_store.insert(Arc::new(MutatingProvider {
                db: ctx.db.clone(),
                workspace_id,
                entity_id,
                delete: false,
                mutate: AtomicBool::new(false),
                width: 768,
            }) as Arc<dyn EmbeddingProvider>);
            let no_content_id = Uuid::now_v7();
            Entity::record_enqueue(
                &ctx.db,
                Enqueue {
                    id: no_content_id,
                    job_name: "embedding_sync",
                    worker_class: WorkerClass::Shared,
                    workspace_id: Some(workspace_id),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .expect("record no-content lifecycle");
            EmbeddingSyncWorkerShared::build(&ctx)
                .perform(EmbeddingSyncArgs {
                    lifecycle_id: Some(no_content_id),
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                })
                .await
                .expect("no content is a no-op");
            let no_content = Entity::find_by_id(no_content_id)
                .one(&ctx.db)
                .await
                .expect("read no-content lifecycle")
                .expect("no-content lifecycle row");
            assert_eq!(no_content.status, LifecycleStatus::Completed.as_db_str());
            assert_eq!(
                no_content.error.as_deref(),
                Some("entity has no x-embed content")
            );

            let (workspace_id, entity_id) = seed(&ctx).await;
            ctx.shared_store
                .insert(Arc::new(DisabledProvider) as Arc<dyn EmbeddingProvider>);
            let disabled_id = Uuid::now_v7();
            Entity::record_enqueue(
                &ctx.db,
                Enqueue {
                    id: disabled_id,
                    job_name: "embedding_sync",
                    worker_class: WorkerClass::Shared,
                    workspace_id: Some(workspace_id),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .expect("record disabled lifecycle");
            EmbeddingSyncWorkerShared::build(&ctx)
                .perform(EmbeddingSyncArgs {
                    lifecycle_id: Some(disabled_id),
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                })
                .await
                .expect("disabled provider is a no-op");
            let disabled = Entity::find_by_id(disabled_id)
                .one(&ctx.db)
                .await
                .expect("read disabled lifecycle")
                .expect("disabled lifecycle row");
            assert_eq!(disabled.status, LifecycleStatus::Completed.as_db_str());
            assert_eq!(
                disabled.error.as_deref(),
                Some("embedding provider explicitly disabled")
            );
        })
        .await;
    }

    /// A write that lands between the worker's snapshot read and the vector persist must not store a stale vector.
    /// The old delivery is terminally superseded; the REST update dispatches the fresh job.
    /// SQLite-only: the live queue it drives is a test-local SQLite file, so the test returns early when `DATABASE_URL` names another backend.
    #[tokio::test]
    #[serial(process_environment)]
    async fn a_write_during_embedding_supersedes_the_old_delivery() {
        if !std::env::var("DATABASE_URL")
            .map(|url| url.starts_with("sqlite:"))
            .unwrap_or(false)
        {
            return;
        }
        boot_request::<App, _, _>(|request, ctx| async move {
            let (workspace_id, entity_id) = seed(&ctx).await;

            let provider = Arc::new(MutatingProvider {
                db: ctx.db.clone(),
                workspace_id,
                entity_id,
                delete: false,
                mutate: AtomicBool::new(true),
                width: 768,
            });
            ctx.shared_store
                .insert(provider.clone() as Arc<dyn EmbeddingProvider>);

            let queue = ctx.queue_provider.clone().expect("booted queue");
            let queue_pool = match ctx.config.queue.as_ref() {
                Some(loco_rs::config::QueueConfig::Sqlite(queue)) => Some(
                    sqlx::SqlitePool::connect(&queue.uri)
                        .await
                        .expect("connect to the SQLite queue file"),
                ),
                _ => None,
            };
            queue.ping().await.expect("ping booted queue");

            let lifecycle_id = Uuid::now_v7();
            Entity::record_enqueue(
                &ctx.db,
                Enqueue {
                    id: lifecycle_id,
                    job_name: "embedding_sync",
                    worker_class: WorkerClass::Shared,
                    workspace_id: None,
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .expect("record lifecycle");
            let args = EmbeddingSyncArgs {
                lifecycle_id: Some(lifecycle_id),
                workspace_id,
                entity_id,
                worker_class: WorkerClass::Shared,
            };
            let status = || async {
                Entity::find_by_id(lifecycle_id)
                    .one(&ctx.db)
                    .await
                    .expect("read lifecycle")
                    .expect("lifecycle row")
                    .status
            };

            EmbeddingSyncWorkerShared::build(&ctx)
                .perform(args.clone())
                .await
                .expect("a superseded delivery is a terminal no-op");

            assert_eq!(status().await, LifecycleStatus::Cancelled.as_db_str());
            if let Some(queue_pool) = &queue_pool {
                let jobs = sqlt::get_jobs(queue_pool, None, None)
                    .await
                    .expect("get SQLite jobs");
                assert_eq!(jobs.len(), 0, "the old delivery must not requeue: {jobs:?}");
            }
            assert_eq!(
                vector_rows(&ctx, entity_id).await,
                0,
                "the stale vector must not be stored"
            );

            let owner = yorishiro::models::user_users::create_user(
                &ctx.db,
                "embedding-update@example.com",
                "test-password-test",
                None,
            )
            .await
            .expect("create update owner");
            let tenant_id = workspace_workspaces::Entity::find_by_id(workspace_id)
                .one(&ctx.db)
                .await
                .expect("read update workspace")
                .expect("update workspace")
                .tenant_id;
            yorishiro::models::tenant_memberships::add_member(
                &ctx.db,
                tenant_id,
                owner.id,
                MembershipRole::Owner,
            )
            .await
            .expect("add update owner");
            let write_key = api_keys::Entity::create_api_key(
                &ctx.db,
                workspace_id,
                ApiKeyScope::Write,
                Some(owner.id),
                false,
            )
            .await
            .expect("create write key")
            .plaintext;
            let fresh_update = request
                .put(&format!("/api/entities/{entity_id}"))
                .add_header("Authorization", format!("Bearer {write_key}"))
                .json(&serde_json::json!({"data": {"title": "current through REST"}}))
                .await;
            assert_eq!(
                fresh_update.status_code(),
                200,
                "REST update: {}",
                fresh_update.text()
            );
            let fresh_jobs = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if let Some(queue_pool) = &queue_pool {
                        let jobs = sqlt::get_jobs(queue_pool, None, None)
                            .await
                            .expect("read fresh SQLite jobs");
                        if !jobs.is_empty() {
                            break Some(jobs);
                        }
                    } else {
                        break None;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("REST update must dispatch a fresh job");
            if let Some(fresh_jobs) = fresh_jobs {
                assert_eq!(
                    fresh_jobs.len(),
                    1,
                    "the newer entity update must dispatch a fresh job; response={}, jobs={fresh_jobs:?}",
                    fresh_update.text()
                );
            }
            queue
                .register(EmbeddingSyncWorkerShared::build(&ctx))
                .await
                .expect("register embedding worker");
            let running = queue.clone();
            let worker =
                tokio::spawn(
                    async move { running.run(vec![WorkerClass::Shared.tag().into()]).await },
                );
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let count = vector_rows(&ctx, entity_id).await;
                    if count == 1 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("fresh delivery persists current data");
            let _ = queue.shutdown();
            worker.await.expect("join worker").expect("run worker");
            assert_eq!(vector_rows(&ctx, entity_id).await, 1);

            let _ = queue.shutdown();
            if let Some(queue_pool) = queue_pool {
                queue_pool.close().await;
            }
        })
        .await;
    }

    /// Legacy queue capacity metadata does not block an embedding worker after process-local admission.
    #[tokio::test]
    async fn legacy_saturated_embedding_metadata_is_ignored() {
        boot_request::<App, _, _>(|_request, ctx| async move {
            let (workspace_id, entity_id) = seed(&ctx).await;
            let enqueue = |id| Enqueue {
                id,
                job_name: "embedding_sync",
                worker_class: WorkerClass::Shared,
                workspace_id: Some(workspace_id),
                plan: None,
                concurrency_key: Some("shared:test"),
                concurrency_limit: Some(1),
            };
            let holder = Uuid::now_v7();
            Entity::record_enqueue(&ctx.db, enqueue(holder))
                .await
                .expect("record holder");
            Entity::start_at(&ctx.db, holder, chrono::Utc::now().fixed_offset())
                .await
                .expect("holder runs");

            let waiting = Uuid::now_v7();
            Entity::record_enqueue(&ctx.db, enqueue(waiting))
                .await
                .expect("record waiting");
            let result = EmbeddingSyncWorkerShared::build(&ctx)
                .perform(EmbeddingSyncArgs {
                    lifecycle_id: Some(waiting),
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                })
                .await;
            assert!(
                result.is_ok(),
                "legacy capacity metadata must not block embedding"
            );
            let row = Entity::find_by_id(waiting)
                .one(&ctx.db)
                .await
                .expect("read waiting")
                .expect("waiting row");
            assert_ne!(row.status, LifecycleStatus::Failed.as_db_str());
            let holder = Entity::find_by_id(holder)
                .one(&ctx.db)
                .await
                .expect("read holder")
                .expect("holder row");
            assert_eq!(holder.status, LifecycleStatus::Running.as_db_str());
        })
        .await;
    }

    /// A payload that fails `serde_json::from_value` never reaches the typed worker, so only the provider job records the failure.
    /// The custom lifecycle row is deliberately not asserted: it is authoritative only after a successful deserialization.
    #[tokio::test]
    async fn a_malformed_payload_fails_the_provider_job_without_requeue() {
        boot_request::<App, _, _>(|_request, ctx| async move {
            let directory = tempfile::tempdir().expect("queue tempdir");
            let queue_uri = format!(
                "sqlite://{}?mode=rwc",
                directory.path().join("queue.sqlite3").display()
            );
            let queue = bgworker::sqlt::create_provider(&SqliteQueueConfig {
                uri: queue_uri.clone(),
                dangerously_flush: false,
                enable_logging: false,
                max_connections: 2,
                min_connections: 1,
                connect_timeout: 5_000,
                idle_timeout: 5_000,
                poll_interval_sec: 1,
                num_workers: 1,
                reaper: None,
            })
            .await
            .expect("SQLite queue");
            queue.setup().await.expect("set up queue");
            let queue = Arc::new(queue);
            let ctx = ctx.into_builder().queue_provider(queue.clone()).build();
            let queue_pool = sqlx::SqlitePool::connect(&queue_uri)
                .await
                .expect("connect to the queue file");

            queue
                .register(EmbeddingSyncWorkerShared::build(&ctx))
                .await
                .expect("register embedding worker");
            for payload in [
                serde_json::json!({"workspace_id": "not-a-uuid", "entity_id": Uuid::now_v7(), "worker_class": "shared"}),
                serde_json::json!({"workspace_id": Uuid::now_v7(), "entity_id": Uuid::now_v7(), "worker_class": "bogus"}),
                serde_json::json!({"workspace_id": Uuid::now_v7(), "worker_class": "shared"}),
            ] {
                queue
                    .enqueue(
                        EmbeddingSyncWorkerShared::class_name(),
                        None,
                        payload,
                        Some(vec![WorkerClass::Shared.tag().into()]),
                        Some(100),
                    )
                    .await
                    .expect("persist malformed payload through the provider");
            }

            let running = queue.clone();
            let worker = tokio::spawn(async move {
                running.run(vec![WorkerClass::Shared.tag().into()]).await
            });
            let failed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let jobs = sqlt::get_jobs(
                        &queue_pool,
                        Some(&vec![loco_rs::bgworker::JobStatus::Failed]),
                        None,
                    )
                    .await
                    .expect("read failed jobs");
                    if jobs.len() == 3 {
                        break jobs;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("malformed jobs must fail in the provider");
            assert_eq!(failed.len(), 3);
            let _ = queue.shutdown();
            worker.await.expect("join worker").expect("run worker");
            assert_eq!(
                sqlt::get_jobs(&queue_pool, None, None)
                    .await
                    .expect("read final queue")
                    .len(),
                3,
                "no replacement job may be enqueued"
            );
            queue_pool.close().await;
        })
        .await;
    }

    /// An entity deleted while it is being embedded is the one zero-row outcome that is not a conflict: there is nothing left to embed.
    /// The job finishes without error and without storing a vector.
    #[tokio::test]
    async fn an_entity_deleted_during_embedding_is_skipped_without_a_vector() {
        boot_request::<App, _, _>(|_request, ctx| async move {
            let (workspace_id, entity_id) = seed(&ctx).await;
            ctx.shared_store.insert(Arc::new(MutatingProvider {
                db: ctx.db.clone(),
                workspace_id,
                entity_id,
                delete: true,
                mutate: AtomicBool::new(true),
                width: 768,
            }) as Arc<dyn EmbeddingProvider>);

            let lifecycle_id = Uuid::now_v7();
            Entity::record_enqueue(
                &ctx.db,
                Enqueue {
                    id: lifecycle_id,
                    job_name: "embedding_sync",
                    worker_class: WorkerClass::Shared,
                    workspace_id: Some(workspace_id),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .expect("record lifecycle");

            EmbeddingSyncWorkerShared::build(&ctx)
                .perform(EmbeddingSyncArgs {
                    lifecycle_id: Some(lifecycle_id),
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                })
                .await
                .expect("a deleted entity is not a failure");

            assert_eq!(vector_rows(&ctx, entity_id).await, 0);
            let lifecycle = Entity::find_by_id(lifecycle_id)
                .one(&ctx.db)
                .await
                .expect("read lifecycle")
                .expect("lifecycle row");
            assert_eq!(lifecycle.status, LifecycleStatus::Completed.as_db_str());
        })
        .await;
    }

    /// Structural failures are recorded as terminal lifecycle failures rather than completed jobs.
    #[tokio::test]
    async fn a_live_dimension_failure_is_not_marked_completed() {
        boot_request::<App, _, _>(|_request, ctx| async move {
            let (workspace_id, entity_id) = seed(&ctx).await;
            let mut workspace = workspace_workspaces::Entity::find_by_id(workspace_id)
                .one(&ctx.db)
                .await
                .expect("read workspace")
                .expect("workspace exists")
                .into_active_model();
            workspace.embedding_dimensions = sea_orm::ActiveValue::Set(Some(768));
            workspace
                .update(&ctx.db)
                .await
                .expect("stamp workspace width");
            ctx.shared_store.insert(Arc::new(MutatingProvider {
                db: ctx.db.clone(),
                workspace_id,
                entity_id,
                delete: false,
                mutate: AtomicBool::new(false),
                width: 1024,
            }) as Arc<dyn EmbeddingProvider>);

            let lifecycle_id = Uuid::now_v7();
            Entity::record_enqueue(
                &ctx.db,
                Enqueue {
                    id: lifecycle_id,
                    job_name: "embedding_sync",
                    worker_class: WorkerClass::Shared,
                    workspace_id: Some(workspace_id),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .expect("record lifecycle");

            let _ = EmbeddingSyncWorkerShared::build(&ctx)
                .perform(EmbeddingSyncArgs {
                    lifecycle_id: Some(lifecycle_id),
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                })
                .await;
            // The lifecycle wrapper records the failure and does not requeue it.

            let lifecycle = Entity::find_by_id(lifecycle_id)
                .one(&ctx.db)
                .await
                .expect("read lifecycle")
                .expect("lifecycle row");
            assert_eq!(lifecycle.status, LifecycleStatus::Failed.as_db_str());
            assert_ne!(lifecycle.status, LifecycleStatus::Completed.as_db_str());
            assert!(lifecycle.error.is_some());
        })
        .await;
    }
}
