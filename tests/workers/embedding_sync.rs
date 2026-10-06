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
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use loco_rs::app::AppContext;
    use loco_rs::bgworker::{self, BackgroundWorker, sqlt};
    use loco_rs::config::SqliteQueueConfig;
    use sea_orm::{
        ActiveModelTrait, DatabaseConnection, EntityTrait, FromQueryResult, IntoActiveModel,
        Statement,
    };
    use uuid::Uuid;
    use yorishiro::App;
    use yorishiro::error::YorishiroError;
    use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
    use yorishiro::models::queue_job_lifecycles::{Enqueue, Entity, LifecycleStatus};
    use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
    use yorishiro::models::{entity_entities, schema_schemas};
    use yorishiro::services::embedding::{EmbedKind, EmbeddingProvider};
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

    /// A write that lands between the worker's snapshot read and the vector persist must not store a stale vector.
    /// The worker reports the conflict through the real lifecycle: the row becomes `retrying`, the job is requeued through a real SQLite queue, and the retry then embeds the current data.
    /// Runs on whichever backend `DATABASE_URL` names, so SQLite and PostgreSQL share one assertion set.
    #[tokio::test]
    async fn a_write_during_embedding_is_requeued_and_the_retry_persists() {
        boot_request::<App, _, _>(|_request, ctx| async move {
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
                .expect("a conflict is requeued, not surfaced");

            assert_eq!(status().await, LifecycleStatus::Retrying.as_db_str());
            let jobs = sqlt::get_jobs(&queue_pool, None, None)
                .await
                .expect("get_jobs");
            assert_eq!(jobs.len(), 1, "jobs: {jobs:?}");
            assert_eq!(jobs[0].name, "EmbeddingSyncWorkerShared");
            assert_eq!(
                vector_rows(&ctx, entity_id).await,
                0,
                "the stale vector must not be stored"
            );

            EmbeddingSyncWorkerShared::build(&ctx)
                .perform(args)
                .await
                .expect("the retry succeeds");

            assert_eq!(status().await, LifecycleStatus::Completed.as_db_str());
            assert_eq!(vector_rows(&ctx, entity_id).await, 1);

            let _ = queue.shutdown();
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

    /// Structural failures are recorded as retryable lifecycle failures rather than completed jobs.
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
            // The lifecycle wrapper records and requeues the failure.

            let lifecycle = Entity::find_by_id(lifecycle_id)
                .one(&ctx.db)
                .await
                .expect("read lifecycle")
                .expect("lifecycle row");
            assert_eq!(lifecycle.status, LifecycleStatus::Retrying.as_db_str());
            assert_ne!(lifecycle.status, LifecycleStatus::Completed.as_db_str());
            assert!(lifecycle.error.is_some());
        })
        .await;
    }
}
