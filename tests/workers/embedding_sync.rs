/// Tests for the embedding sync worker: routing, serialization, and class names.
use loco_rs::bgworker::BackgroundWorker;
use yorishiro::workers::embedding_sync::{EmbeddingSyncWorker, WorkerClass};

/// The worker declares neither tags nor a queue: a job's target is chosen when the job is enqueued, and a process chooses its targets when it starts.
/// A tag here would put a second, fixed tag on every job and let a worker for another target match on it.
#[test]
fn the_worker_declares_no_routing_of_its_own() {
    assert!(EmbeddingSyncWorker::tags().is_empty());
    assert!(EmbeddingSyncWorker::queue().is_none());
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

/// Loco stores the class name on every queued job and finds the handler by it, so renaming the worker strands whatever is already queued.
#[test]
fn the_worker_keeps_its_queue_class_name() {
    assert_eq!(EmbeddingSyncWorker::class_name(), "EmbeddingSyncWorker");
}

mod conflict {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::FutureExt;
    use loco_rs::app::AppContext;
    use loco_rs::bgworker::{self, BackgroundWorker};
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use tokio::sync::{Notify, Semaphore};
    use uuid::Uuid;
    use yorishiro::App;
    use yorishiro::models::_entities::{api_keys, workspace_workspaces};
    use yorishiro::models::api_keys::ApiKeyScope;
    use yorishiro::models::queue_job_lifecycles::{Entity, LifecycleStatus};
    use yorishiro::models::tenant_memberships::MembershipRole;
    use yorishiro::models::{entity_entities, schema_schemas};
    use yorishiro::services::embedding::{
        EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    };
    use yorishiro::workers::embedding_sync::{EmbeddingSyncArgs, EmbeddingSyncWorker, WorkerClass};
    use yorishiro::workers::target::WorkerTarget;

    use crate::requests::boot_request_isolated as boot_request;

    struct BackendObservation(Arc<bgworker::Queue>);

    impl BackendObservation {
        async fn owned_statuses(
            &self,
            ids: &[String],
        ) -> Vec<(String, loco_rs::bgworker::JobStatus)> {
            let directory = tempfile::tempdir().unwrap();
            let dump = self.0.dump(directory.path(), None, None).await.unwrap();
            let jobs: Vec<loco_rs::bgworker::Job> =
                serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
            jobs.into_iter()
                .filter(|job| ids.contains(&job.id))
                .map(|job| (job.id, job.status))
                .collect()
        }

        async fn owned_count(
            &self,
            ids: &[String],
            status: Option<Vec<loco_rs::bgworker::JobStatus>>,
        ) -> usize {
            self.owned_statuses(ids)
                .await
                .into_iter()
                .filter(|(_, found)| status.as_ref().is_none_or(|wanted| wanted.contains(found)))
                .count()
        }
    }

    struct QueueHarness {
        queue: Arc<bgworker::Queue>,
        ctx: AppContext,
        observation: BackendObservation,
        runner: tokio::task::JoinHandle<loco_rs::Result<()>>,
    }

    impl QueueHarness {
        async fn lane(mut ctx: AppContext, workers: u32) -> Self {
            use loco_rs::app::Hooks;
            match ctx.config.queue.as_mut().expect("configured queue") {
                loco_rs::config::QueueConfig::Sqlite(config) => config.num_workers = workers,
                loco_rs::config::QueueConfig::Postgres(config) => config.num_workers = workers,
                loco_rs::config::QueueConfig::Redis(config) => config.num_workers = workers,
                _ => unreachable!(),
            }
            let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
            ctx.queue_provider = Some(queue.clone());
            App::connect_workers(&ctx, &queue).await.unwrap();
            let running = queue.clone();
            let runner =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            Self {
                observation: BackendObservation(queue.clone()),
                queue,
                ctx,
                runner,
            }
        }

        async fn enqueue(&self, workspace_id: Uuid, entity_id: Uuid) -> (Uuid, String) {
            yorishiro::workers::embedding_sync::enqueue_for_class(
                &self.ctx,
                EmbeddingSyncArgs {
                    lifecycle_id: None,
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                },
            )
            .await
            .expect("enqueue through production dispatch");
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    use yorishiro::models::_entities::queue_job_dispatch_outbox as outbox;
                    let rows = outbox::Entity::find()
                        .filter(outbox::Column::WorkspaceId.eq(workspace_id))
                        .all(&self.ctx.db)
                        .await
                        .expect("read this workspace's durable dispatches");
                    if let Some(row) = rows.iter().find(|row| {
                        row.payload["entity_id"] == entity_id.to_string()
                            && row.published_at.is_some()
                    }) {
                        let lifecycle_id = row.lifecycle_id.expect("embedding lifecycle ID");
                        let lifecycle = Entity::find_by_id(lifecycle_id)
                            .one(&self.ctx.db)
                            .await
                            .expect("read published lifecycle")
                            .expect("published lifecycle exists");
                        return (
                            lifecycle_id,
                            lifecycle
                                .provider_job_id
                                .expect("relay records provider job ID"),
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("production relay publishes the lifecycle")
        }

        async fn shutdown(self) {
            use loco_rs::app::Hooks;
            App::on_shutdown(&self.ctx).await;
            let shutdown = self.queue.shutdown();
            let joined =
                tokio::time::timeout(std::time::Duration::from_secs(10), self.runner).await;
            let closed = self.queue.close().await;
            shutdown.expect("shut down embedding queue");
            joined
                .expect("embedding workers drain before timeout")
                .expect("embedding worker task joins")
                .expect("embedding worker run succeeds");
            closed.expect("close embedding queue");
        }
    }

    enum FailureKind {
        Busy,
        Unreachable,
    }

    async fn failing_provider(
        kind: FailureKind,
    ) -> (Arc<dyn EmbeddingProvider>, crate::http_stub::HttpStub) {
        let server = match kind {
            FailureKind::Busy => {
                crate::http_stub::HttpStub::start(axum::Router::new().route(
                    "/embeddings",
                    axum::routing::post(|| async {
                        (
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                            [("Retry-After", "1")],
                            "private provider diagnostic",
                        )
                    }),
                ))
                .await
            }
            FailureKind::Unreachable => {
                crate::http_stub::HttpStub::raw("not an HTTP response\r\n\r\n".into()).await
            }
        };
        let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "failure-http-test".into(),
            dimensions: 768,
            send_dimensions_param: false,
        }));
        (provider, server)
    }

    async fn disabled_provider(ctx: &AppContext) -> Arc<dyn EmbeddingProvider> {
        let mut settings = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .expect("deployment settings");
        settings.embedding = serde_json::from_value(serde_json::json!({
            "provider": "none", "dimensions": 768, "api_key": "",
            "send_dimensions_param": false, "local_model": "unused",
            "local_max_sequence_length": 512
        }))
        .expect("disabled embedding settings");
        yorishiro::services::embedding::build_embedding_provider(&settings)
            .await
            .expect("build disabled production provider")
    }

    struct GatedProvider {
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        entered: Notify,
        release: Semaphore,
        width: usize,
    }

    struct AggregateGatedProvider {
        active: AtomicUsize,
        max_active: AtomicUsize,
        aggregate: Arc<AtomicUsize>,
        aggregate_max: Arc<AtomicUsize>,
        entered: Arc<Notify>,
        release: Arc<Semaphore>,
    }

    impl AggregateGatedProvider {
        async fn transport(
            self: &Arc<Self>,
        ) -> (Arc<dyn EmbeddingProvider>, crate::http_stub::HttpStub) {
            let observed = self.clone();
            let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
                "/embeddings", axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                    let state = observed.clone();
                    async move {
                        let local = state.active.fetch_add(1, Ordering::SeqCst) + 1;
                        state.max_active.fetch_max(local, Ordering::SeqCst);
                        let aggregate = state.aggregate.fetch_add(1, Ordering::SeqCst) + 1;
                        state.aggregate_max.fetch_max(aggregate, Ordering::SeqCst);
                        state.entered.notify_waiters();
                        let _local_guard = ActiveGuard(&state.active);
                        let _aggregate_guard = ActiveGuard(&state.aggregate);
                        state.release.acquire().await.unwrap().forget();
                        let data = body["input"].as_array().unwrap().iter().enumerate()
                            .map(|(index, _)| serde_json::json!({"index":index,"embedding":vec![1.0_f32;768]}))
                            .collect::<Vec<_>>();
                        axum::Json(serde_json::json!({"data":data}))
                    }
                }),
            )).await;
            let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: server.url.clone(),
                api_key: "test-key".into(),
                model: "aggregate-context-provider".into(),
                dimensions: 768,
                send_dimensions_param: false,
            }));
            (provider, server)
        }
    }

    struct ErrorThenSuccessProvider {
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        entered: Notify,
        release: Semaphore,
    }

    impl ErrorThenSuccessProvider {
        async fn start() -> (
            Arc<Self>,
            Arc<dyn EmbeddingProvider>,
            crate::http_stub::HttpStub,
        ) {
            use axum::response::IntoResponse;

            let state = Arc::new(Self {
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                entered: Notify::new(),
                release: Semaphore::new(0),
            });
            let observed = state.clone();
            let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
                "/embeddings",
                axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                    let state = observed.clone();
                    async move {
                        let call = state.calls.fetch_add(1, Ordering::SeqCst);
                        let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
                        state.max_active.fetch_max(active, Ordering::SeqCst);
                        let _guard = ActiveGuard(&state.active);
                        state.entered.notify_waiters();
                        state.release.acquire().await.unwrap().forget();
                        if call == 0 {
                            return (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                [("Retry-After", "1")],
                                "private scripted provider failure",
                            ).into_response();
                        }
                        let data = body["input"].as_array().unwrap().iter().enumerate()
                            .map(|(index, _)| serde_json::json!({"index": index, "embedding": vec![1.0_f32; 768]}))
                            .collect::<Vec<_>>();
                        axum::Json(serde_json::json!({"data": data})).into_response()
                    }
                }),
            )).await;
            let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: server.url.clone(),
                api_key: "test-key".into(),
                model: "error-then-success-provider".into(),
                dimensions: 768,
                send_dimensions_param: false,
            }));
            (state, provider, server)
        }
    }

    struct MixedProvider {
        active: AtomicUsize,
        max_active: AtomicUsize,
        entered: Notify,
        release: Semaphore,
        order: Mutex<Vec<String>>,
    }

    struct ActiveGuard<'a>(&'a AtomicUsize);

    impl Drop for ActiveGuard<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl MixedProvider {
        async fn start() -> (
            Arc<Self>,
            Arc<dyn EmbeddingProvider>,
            crate::http_stub::HttpStub,
        ) {
            let state = Arc::new(Self {
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                entered: Notify::new(),
                release: Semaphore::new(0),
                order: Mutex::new(Vec::new()),
            });
            let observed = state.clone();
            let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
                "/embeddings", axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                    let state = observed.clone();
                    async move {
                        let inputs = body["input"].as_array().unwrap();
                        state.order.lock().unwrap().extend(inputs.iter().map(|input| input.as_str().unwrap().to_owned()));
                        let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
                        state.max_active.fetch_max(active, Ordering::SeqCst);
                        let _guard = ActiveGuard(&state.active);
                        state.entered.notify_waiters();
                        state.release.acquire().await.unwrap().forget();
                        let data = inputs.iter().enumerate().map(|(index, _)|
                            serde_json::json!({"index":index,"embedding":vec![1.0_f32;768]})).collect::<Vec<_>>();
                        axum::Json(serde_json::json!({"data":data}))
                    }
                }),
            )).await;
            let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: server.url.clone(),
                api_key: "test-key".into(),
                model: "mixed-test-provider".into(),
                dimensions: 768,
                send_dimensions_param: false,
            }));
            (state, provider, server)
        }
    }

    impl GatedProvider {
        async fn start(
            permits: usize,
        ) -> (
            Arc<Self>,
            Arc<dyn EmbeddingProvider>,
            crate::http_stub::HttpStub,
        ) {
            let state = Arc::new(Self {
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                entered: Notify::new(),
                release: Semaphore::new(permits),
                width: 768,
            });
            let observed = state.clone();
            let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
                "/embeddings", axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                    let state = observed.clone();
                    async move {
                        state.calls.fetch_add(1, Ordering::SeqCst);
                        let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
                        state.max_active.fetch_max(active, Ordering::SeqCst);
                        let _guard = ActiveGuard(&state.active);
                        state.entered.notify_waiters();
                        state.release.acquire().await.unwrap().forget();
                        let data = body["input"].as_array().unwrap().iter().enumerate()
                            .map(|(index, _)| serde_json::json!({"index":index,"embedding":vec![1.0_f32;state.width]}))
                            .collect::<Vec<_>>();
                        axum::Json(serde_json::json!({"data":data}))
                    }
                }),
            )).await;
            let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: server.url.clone(),
                api_key: "test-key".into(),
                model: "gated-test-provider".into(),
                dimensions: 768,
                send_dimensions_param: false,
            }));
            (state, provider, server)
        }
    }

    async fn vector_provider(
        width: usize,
        gated: bool,
    ) -> (
        Arc<dyn EmbeddingProvider>,
        crate::http_stub::HttpStub,
        Arc<Notify>,
        Arc<Semaphore>,
    ) {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(if gated { 0 } else { 1 }));
        let observed = entered.clone();
        let gate = release.clone();
        let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
            "/embeddings", axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let entered = observed.clone();
                let release = gate.clone();
                async move {
                    entered.notify_one();
                    let _permit = release.acquire().await.unwrap();
                    let data = body["input"].as_array().unwrap().iter().enumerate().map(|(index, _)|
                        serde_json::json!({"index":index,"embedding":vec![1.0_f32;width]})).collect::<Vec<_>>();
                    axum::Json(serde_json::json!({"data":data}))
                }
            }),
        )).await;
        let provider = Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "snapshot-http-provider".into(),
            dimensions: width,
            send_dimensions_param: false,
        }));
        (provider, server, entered, release)
    }

    async fn seed(request: &loco_rs::TestServer, ctx: &AppContext) -> (Uuid, Uuid) {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(request, ctx, Default::default()).await;
        seed_embeddable_entity(ctx, tenant_id, workspace_id).await
    }

    async fn seed_embeddable_entity(
        ctx: &AppContext,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> (Uuid, Uuid) {
        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": { "note": { "fields": {
                "title": { "type": "string", "required": true, "x-embed": true }
            } } }
        }))
        .expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, definition, None, None)
            .await
            .expect("create schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "original" }),
            },
            None,
        )
        .await
        .expect("create entity");
        (workspace_id, entity.id)
    }

    async fn seed_without_embeddable_content(
        request: &loco_rs::TestServer,
        ctx: &AppContext,
    ) -> (Uuid, Uuid) {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(request, ctx, Default::default()).await;
        seed_plain_entity(ctx, tenant_id, workspace_id).await
    }

    async fn seed_plain_entity(
        ctx: &AppContext,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> (Uuid, Uuid) {
        let definition = serde_json::from_value(serde_json::json!({
            "name": "plain",
            "entity_types": { "plain": { "fields": {
                "title": { "type": "string", "required": true }
            } } }
        }))
        .expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, definition, None, None)
            .await
            .expect("create schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "plain".into(),
                entity_type: "plain".into(),
                data: serde_json::json!({ "title": "not embedded" }),
            },
            None,
        )
        .await
        .expect("create entity");
        (workspace_id, entity.id)
    }

    async fn vector_rows(ctx: &AppContext, entity_id: Uuid) -> i64 {
        use yorishiro::models::_entities::entity_embeddings_768::{Column, Entity};
        Entity::find()
            .filter(Column::EntityId.eq(entity_id))
            .count(&ctx.db)
            .await
            .expect("count vectors")
            .try_into()
            .expect("vector count fits i64")
    }

    #[tokio::test]
    async fn provider_failures_are_terminal_and_do_not_requeue() {
        use loco_rs::{app::Hooks, bgworker::JobStatus};
        for kind in [FailureKind::Busy, FailureKind::Unreachable] {
            boot_request::<App, _, _>(|request, ctx| async move {
                let (workspace_id, entity_id) = seed(&request, &ctx).await;
                let (provider, _server) = failing_provider(kind).await;
                ctx.shared_store.insert(provider);
                let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
                let mut worker_ctx = ctx.clone();
                worker_ctx.queue_provider = Some(queue.clone());
                App::connect_workers(&worker_ctx, &queue).await.unwrap();
                yorishiro::workers::embedding_sync::enqueue_for_class(
                    &ctx,
                    EmbeddingSyncArgs {
                        lifecycle_id: None,
                        workspace_id,
                        entity_id,
                        worker_class: WorkerClass::Shared,
                    },
                ).await.unwrap();
                let running = queue.clone();
                let worker = tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
                let directory = tempfile::tempdir().unwrap();
                let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    let lifecycle_id = loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> = serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        let owned: Vec<_> = jobs.iter().filter(|job| job.data["entity_id"] == entity_id.to_string()
                            && job.data["workspace_id"] == workspace_id.to_string()).collect();
                        if owned.iter().any(|job| job.status == JobStatus::Failed) {
                            assert_eq!(owned.len(), 1, "provider failure must not requeue");
                            break serde_json::from_value::<Uuid>(owned[0].data["lifecycle_id"].clone()).unwrap();
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    };
                    let lifecycle = Entity::find_by_id(lifecycle_id).one(&ctx.db).await.unwrap().unwrap();
                    assert_eq!(lifecycle.status, LifecycleStatus::Failed.as_db_str());
                    assert!(lifecycle.error.is_some());
                    // A terminal lifecycle with an acknowledged outbox cannot be relayed again.
                    let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(lifecycle_id).one(&ctx.db).await.unwrap().unwrap();
                    assert!(outbox.published_at.is_some());
                })).catch_unwind().await;
                App::on_shutdown(&worker_ctx).await;
                queue.shutdown().unwrap();
                let drained = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
                queue.close().await.unwrap();
                match outcome {
                    Ok(result) => result.expect("configured queue records provider failure"),
                    Err(panic) => std::panic::resume_unwind(panic),
                }
                drained.unwrap().unwrap().unwrap();
            }).await;
        }
    }

    #[tokio::test]
    async fn three_document_worker_entries_share_the_installed_limiter() {
        boot_request::<App, _, _>(|request, ctx| async move {
            let (workspace_id, first) = seed(&request, &ctx).await;
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
            let (provider, transport, _server) = GatedProvider::start(0).await;
            ctx.shared_store.insert(transport.clone());
            let harness = QueueHarness::lane(ctx, 3).await;
            let ctx = harness.ctx.clone();
            let ids = [first, second, third];
            let mut lifecycle_ids = Vec::new();
            let mut queue_ids = Vec::new();
            for entity_id in ids {
                let (lifecycle_id, queue_id) = harness.enqueue(workspace_id, entity_id).await;
                lifecycle_ids.push(lifecycle_id);
                queue_ids.push(queue_id);
            }
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while provider.calls.load(Ordering::SeqCst) == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("first provider call entered");
            let processing = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let count = harness
                        .observation
                        .owned_count(
                            &queue_ids,
                            Some(vec![loco_rs::bgworker::JobStatus::Processing]),
                        )
                        .await;
                    if count == 3 {
                        break count;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await;
            let processing = match processing {
                Ok(count) => count,
                Err(_) => panic!(
                    "expected all 3 owned jobs processing; jobs={:?}, provider calls={}, active={}, max_active={}",
                    harness.observation.owned_statuses(&queue_ids).await,
                    provider.calls.load(Ordering::SeqCst),
                    provider.active.load(Ordering::SeqCst),
                    provider.max_active.load(Ordering::SeqCst),
                ),
            };
            assert_eq!(processing, 3);
            assert_eq!(provider.active.load(Ordering::SeqCst), 1);
            assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
            let rows = Entity::find()
                .filter(yorishiro::models::queue_job_lifecycles::Column::Id.is_in(lifecycle_ids.clone()))
                .all(&ctx.db)
                .await
                .expect("read lifecycles");
            let in_status = |status: LifecycleStatus| {
                rows.iter()
                    .filter(|row| {
                        lifecycle_ids.contains(&row.id) && row.status == status.as_db_str()
                    })
                    .count()
            };
            assert_eq!(in_status(LifecycleStatus::Running), 1);
            assert_eq!(in_status(LifecycleStatus::Queued), 2);
            assert_eq!(in_status(LifecycleStatus::Failed), 0);
            assert_eq!(harness.observation.owned_count(&queue_ids, None).await, 3);
            provider.release.add_permits(3);
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let done = Entity::find()
                        .filter(yorishiro::models::queue_job_lifecycles::Column::Id.is_in(lifecycle_ids.clone()))
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
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("all queued jobs complete");
            assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
            assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
            assert_eq!(vector_rows(&ctx, first).await, 1);
            assert_eq!(vector_rows(&ctx, second).await, 1);
            assert_eq!(vector_rows(&ctx, third).await, 1);
            harness.shutdown().await;
        })
        .await;
    }

    #[tokio::test]
    async fn document_and_query_workers_share_fifo_embedding_admission() {
        use loco_rs::{app::Hooks, bgworker::JobStatus};
        boot_request::<App, _, _>(|request, ctx| async move {
            let (workspace_id, entity_id) = seed(&request, &ctx).await;
            let (provider, transport, _server) = MixedProvider::start().await;
            ctx.shared_store.insert(transport);
            let mut worker_ctx = ctx.clone();
            match worker_ctx.config.queue.as_mut().unwrap() {
                loco_rs::config::QueueConfig::Sqlite(config) => config.num_workers = 2,
                loco_rs::config::QueueConfig::Postgres(config) => config.num_workers = 2,
                loco_rs::config::QueueConfig::Redis(config) => config.num_workers = 2,
                _ => unreachable!(),
            }
            let queue = crate::requests::query_worker::shared_target_queue(&worker_ctx).await;
            worker_ctx.queue_provider = Some(queue.clone());
            yorishiro::workers::embedding_sync::enqueue_for_class(
                &ctx,
                EmbeddingSyncArgs {
                    lifecycle_id: None,
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                },
            )
            .await
            .unwrap();
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            let directory = tempfile::tempdir().unwrap();
            let outcome = std::panic::AssertUnwindSafe(async {
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        if jobs
                            .iter()
                            .any(|job| job.data["entity_id"] == entity_id.to_string())
                        {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("production relay publishes document");
            })
            .catch_unwind()
            .await;
            if let Err(panic) = outcome {
                App::on_shutdown(&worker_ctx).await;
                queue.close().await.unwrap();
                std::panic::resume_unwind(panic);
            }
            let running = queue.clone();
            let worker =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            let outcome = std::panic::AssertUnwindSafe(async {
                tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    while provider.active.load(Ordering::SeqCst) != 1 {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("document acquired first");
                let tenant_id = workspace_workspaces::Entity::find_by_id(workspace_id)
                    .one(&ctx.db)
                    .await
                    .unwrap()
                    .unwrap()
                    .tenant_id;
                let query_ctx = ctx.clone();
                let mut query = tokio::spawn(async move {
                    yorishiro::workers::query_embedding::embed_query(
                        &query_ctx,
                        tenant_id,
                        workspace_id,
                        "alpha",
                    )
                    .await
                });
                let request_id = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        if query.is_finished() {
                            panic!("query caller finished before its delivery was observed processing: {:?}", (&mut query).await);
                        }
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        if let Some(job) = jobs.iter().find(|job| {
                            job.data["workspace_id"] == workspace_id.to_string()
                                && job.data.get("request_id").is_some()
                                && job.status == JobStatus::Processing
                        }) {
                            break serde_json::from_value::<Uuid>(job.data["request_id"].clone())
                                .unwrap();
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await;
                let request_id = match request_id {
                    Ok(id) => id,
                    Err(error) => {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        panic!("query delivery was not observed processing: {error}; caller_finished={}; queue={}", query.is_finished(), std::fs::read_to_string(dump).unwrap());
                    }
                };
                assert_eq!(provider.active.load(Ordering::SeqCst), 1);
                assert_eq!(provider.order.lock().unwrap().len(), 1);
                assert!(provider.order.lock().unwrap()[0].contains("original"));
                provider.release.add_permits(1);
                tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    while provider.order.lock().unwrap().len() < 2 {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("query is admitted after document");
                let inputs = provider.order.lock().unwrap().clone();
                assert_eq!(inputs.len(), 2);
                assert!(inputs[0].contains("original"));
                assert_eq!(inputs[1], "alpha");
                provider.release.add_permits(1);
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        let document = jobs.iter().find(|job| {
                            job.data["entity_id"] == entity_id.to_string()
                                && job.status == JobStatus::Completed
                        });
                        let query = jobs.iter().any(|job| {
                            job.data["request_id"] == request_id.to_string()
                                && job.status == JobStatus::Completed
                        });
                        if let Some(document) = document.filter(|_| query) {
                            let lifecycle_id =
                                Uuid::parse_str(document.data["lifecycle_id"].as_str().unwrap())
                                    .unwrap();
                            let lifecycle = Entity::find_by_id(lifecycle_id)
                                .one(&ctx.db)
                                .await
                                .unwrap()
                                .unwrap();
                            assert_eq!(lifecycle.status, LifecycleStatus::Completed.as_db_str());
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("both actual queue deliveries complete");
                assert_eq!(query.await.unwrap().unwrap().len(), 768);
                assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
            })
            .catch_unwind()
            .await;
            provider.release.add_permits(2);
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
            queue.close().await.unwrap();
            if let Err(panic) = outcome {
                std::panic::resume_unwind(panic);
            }
            drained.unwrap().unwrap().unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn provider_error_releases_real_worker_permit_without_replacement() {
        use loco_rs::{app::Hooks, bgworker::JobStatus};
        boot_request::<App, _, _>(|request, ctx| async move {
            let (workspace_id, entity_id) = seed(&request, &ctx).await;
            let (provider, transport, _server) = ErrorThenSuccessProvider::start().await;
            ctx.shared_store.insert(transport);
            let mut worker_ctx = ctx.clone();
            match worker_ctx.config.queue.as_mut().unwrap() {
                loco_rs::config::QueueConfig::Sqlite(config) => config.num_workers = 2,
                loco_rs::config::QueueConfig::Postgres(config) => config.num_workers = 2,
                loco_rs::config::QueueConfig::Redis(config) => config.num_workers = 2,
                _ => unreachable!(),
            }
            let queue = crate::requests::query_worker::shared_target_queue(&worker_ctx).await;
            worker_ctx.queue_provider = Some(queue.clone());
            yorishiro::workers::embedding_sync::enqueue_for_class(
                &ctx,
                EmbeddingSyncArgs {
                    lifecycle_id: None,
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                },
            )
            .await
            .unwrap();
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            let directory = tempfile::tempdir().unwrap();
            let published = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let dump = queue.dump(directory.path(), None, None).await.unwrap();
                    let jobs: Vec<loco_rs::bgworker::Job> =
                        serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                    if let Some(job) = jobs.iter().find(|job| {
                        job.data["entity_id"] == entity_id.to_string()
                            && job.data["workspace_id"] == workspace_id.to_string()
                    }) {
                        break serde_json::from_value::<Uuid>(job.data["lifecycle_id"].clone())
                            .unwrap();
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await;
            let failed_id = match published {
                Ok(id) => id,
                Err(_) => {
                    App::on_shutdown(&worker_ctx).await;
                    queue.close().await.unwrap();
                    panic!("production relay did not publish the first delivery");
                }
            };
            let running = queue.clone();
            let worker =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                async {
                    tokio::time::timeout(std::time::Duration::from_secs(1), async {
                        while provider.active.load(Ordering::SeqCst) != 1 {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    })
                    .await
                    .expect("first delivery acquired the embedding permit");
                    yorishiro::workers::embedding_sync::enqueue_for_class(
                        &ctx,
                        EmbeddingSyncArgs {
                            lifecycle_id: None,
                            workspace_id,
                            entity_id,
                            worker_class: WorkerClass::Shared,
                        },
                    )
                    .await
                    .unwrap();
                    let waiting_id = loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        if let Some(job) = jobs.iter().find(|job| {
                            job.data["entity_id"] == entity_id.to_string()
                                && job.data["workspace_id"] == workspace_id.to_string()
                                && job.data["lifecycle_id"] != failed_id.to_string()
                        }) {
                            break serde_json::from_value::<Uuid>(job.data["lifecycle_id"].clone())
                                .unwrap();
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    };
                    loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        let processing = jobs
                            .iter()
                            .filter(|job| {
                                [failed_id.to_string(), waiting_id.to_string()].contains(
                                    &job.data["lifecycle_id"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_owned(),
                                ) && job.status == JobStatus::Processing
                            })
                            .count();
                        if processing == 2 && provider.active.load(Ordering::SeqCst) == 1 {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
                    provider.release.add_permits(1);
                    loop {
                        let row = Entity::find_by_id(failed_id)
                            .one(&ctx.db)
                            .await
                            .unwrap()
                            .unwrap();
                        if row.status == LifecycleStatus::Failed.as_db_str() {
                            assert!(row.error.is_some_and(|error| error.contains("HTTP 503")
                                && !error.contains("private scripted provider failure")));
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    provider.release.add_permits(1);
                    loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        let failed = jobs.iter().any(|job| {
                            job.data["lifecycle_id"] == failed_id.to_string()
                                && job.status == JobStatus::Failed
                        });
                        let completed = jobs.iter().any(|job| {
                            job.data["lifecycle_id"] == waiting_id.to_string()
                                && job.status == JobStatus::Completed
                        });
                        if failed && completed {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    let row = Entity::find_by_id(waiting_id)
                        .one(&ctx.db)
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(row.status, LifecycleStatus::Completed.as_db_str());
                    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
                    assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
                },
            ))
            .catch_unwind()
            .await;
            provider.release.add_permits(2);
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
            queue.close().await.unwrap();
            match outcome {
                Ok(result) => {
                    result.expect("failed delivery releases the permit to the next actual worker")
                }
                Err(panic) => std::panic::resume_unwind(panic),
            }
            drained.unwrap().unwrap().unwrap();
        })
        .await;
    }

    async fn run_backend_admission_scenario_inner(
        request: &loco_rs::TestServer,
        harness: &QueueHarness,
        provider: Arc<GatedProvider>,
    ) {
        let (workspace_id, first) = seed(request, &harness.ctx).await;
        let second = entity_entities::create(
            &harness.ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title":"second"}),
            },
            None,
        )
        .await
        .unwrap();
        let mut ids = Vec::new();
        let mut queue_ids = Vec::new();
        for entity_id in [first, second.id] {
            let (id, queue_id) = harness.enqueue(workspace_id, entity_id).await;
            ids.push(id);
            queue_ids.push(queue_id);
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while provider.active.load(Ordering::SeqCst) != 1 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("external provider holds the first delivery");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while harness
                .observation
                .owned_count(
                    &queue_ids,
                    Some(vec![loco_rs::bgworker::JobStatus::Processing]),
                )
                .await
                < 2
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let rows = Entity::find()
            .filter(yorishiro::models::queue_job_lifecycles::Column::Id.is_in(ids.clone()))
            .all(&harness.ctx.db)
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter(|r| ids.contains(&r.id) && r.status == LifecycleStatus::Running.as_db_str())
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|r| ids.contains(&r.id) && r.status == LifecycleStatus::Queued.as_db_str())
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|r| ids.contains(&r.id) && r.status == LifecycleStatus::Failed.as_db_str())
                .count(),
            0
        );
        assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
        assert_eq!(harness.observation.owned_count(&queue_ids, None).await, 2);
        provider.release.add_permits(2);
        let completed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let rows = Entity::find()
                    .filter(yorishiro::models::queue_job_lifecycles::Column::Id.is_in(ids.clone()))
                    .all(&harness.ctx.db)
                    .await
                    .unwrap();
                if ids.iter().all(|id| {
                    rows.iter()
                        .any(|r| r.id == *id && r.status == LifecycleStatus::Completed.as_db_str())
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        let final_rows = Entity::find()
            .filter(yorishiro::models::queue_job_lifecycles::Column::Id.is_in(ids.clone()))
            .all(&harness.ctx.db)
            .await
            .unwrap();
        assert!(
            completed.is_ok(),
            "owned lifecycles did not complete: {final_rows:?}; HTTP calls={}, active={}",
            provider.calls.load(Ordering::SeqCst),
            provider.active.load(Ordering::SeqCst),
        );
        assert_eq!(harness.observation.owned_count(&queue_ids, None).await, 2);
        assert_eq!(final_rows.iter().filter(|r| ids.contains(&r.id)).count(), 2);
    }

    async fn run_backend_admission_scenario(
        request: &loco_rs::TestServer,
        harness: QueueHarness,
        provider: Arc<GatedProvider>,
    ) {
        let result = std::panic::AssertUnwindSafe(run_backend_admission_scenario_inner(
            request, &harness, provider,
        ))
        .catch_unwind()
        .await;
        harness.shutdown().await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test]
    async fn independently_booted_contexts_keep_independent_embedding_limits() {
        if !crate::require_sqlite_backend() {
            return;
        }
        let aggregate = Arc::new(AtomicUsize::new(0));
        let aggregate_max = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let provider_a = Arc::new(AggregateGatedProvider {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            aggregate: aggregate.clone(),
            aggregate_max: aggregate_max.clone(),
            entered: entered.clone(),
            release: release.clone(),
        });
        boot_request::<App, _, _>(|request, ctx_a| async move {
            if !matches!(
                ctx_a.config.queue,
                Some(loco_rs::config::QueueConfig::Sqlite(_))
            ) {
                return;
            }
            let (workspace, entity) = seed(&request, &ctx_a).await;
            let (transport, _server_a) = provider_a.transport().await;
            ctx_a.shared_store.insert(transport.clone());
            let harness_a = QueueHarness::lane(ctx_a, 2).await;
            let result = std::panic::AssertUnwindSafe(async {
                let (id_a, _) = harness_a.enqueue(workspace, entity).await;
                tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
                    .await
                    .unwrap();
                let provider_b = Arc::new(AggregateGatedProvider {
                    active: AtomicUsize::new(0),
                    max_active: AtomicUsize::new(0),
                    aggregate: aggregate.clone(),
                    aggregate_max: aggregate_max.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                });
                boot_request::<App, _, _>(|request_b, ctx_b| async move {
                    let (workspace_b, entity_b) = seed(&request_b, &ctx_b).await;
                    let (transport, _server_b) = provider_b.transport().await;
                    ctx_b.shared_store.insert(transport.clone());
                    let harness_b = QueueHarness::lane(ctx_b, 2).await;
                    let result_b = std::panic::AssertUnwindSafe(async {
                        let (id_b, _) = harness_b.enqueue(workspace_b, entity_b).await;
                        tokio::time::timeout(std::time::Duration::from_secs(3), async {
                            while aggregate.load(Ordering::SeqCst) < 2 {
                                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            }
                        })
                        .await
                        .unwrap();
                        release.add_permits(2);
                        tokio::time::timeout(std::time::Duration::from_secs(5), async {
                            while Entity::find_by_id(id_b)
                                .one(&harness_b.ctx.db)
                                .await
                                .unwrap()
                                .unwrap()
                                .status
                                != LifecycleStatus::Completed.as_db_str()
                            {
                                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            }
                        })
                        .await
                        .unwrap();
                        assert_eq!(provider_b.max_active.load(Ordering::SeqCst), 1);
                    })
                    .catch_unwind()
                    .await;
                    harness_b.shutdown().await;
                    if let Err(panic) = result_b {
                        std::panic::resume_unwind(panic);
                    }
                })
                .await;
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while Entity::find_by_id(id_a)
                        .one(&harness_a.ctx.db)
                        .await
                        .unwrap()
                        .unwrap()
                        .status
                        != LifecycleStatus::Completed.as_db_str()
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(provider_a.max_active.load(Ordering::SeqCst), 1);
                assert_eq!(aggregate_max.load(Ordering::SeqCst), 2);
            })
            .catch_unwind()
            .await;
            harness_a.shutdown().await;
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        })
        .await;
    }

    #[tokio::test]
    async fn configured_queue_deliveries_wait_for_embedding_capacity() {
        boot_request::<App, _, _>(|request, ctx| async move {
            let (provider, transport, _server) = GatedProvider::start(0).await;
            ctx.shared_store.insert(transport.clone());
            run_backend_admission_scenario(&request, QueueHarness::lane(ctx, 2).await, provider)
                .await;
        })
        .await;
    }

    #[tokio::test]
    async fn no_content_and_disabled_provider_are_successful_noops() {
        for disabled in [false, true] {
            boot_request::<App, _, _>(|request, ctx| async move {
                use loco_rs::{app::Hooks, bgworker::JobStatus};
                let (workspace_id, entity_id) = if disabled {
                    seed(&request, &ctx).await
                } else {
                    seed_without_embeddable_content(&request, &ctx).await
                };
                let (provider, _server, _, _) = vector_provider(768, false).await;
                ctx.shared_store.insert(if disabled {
                    disabled_provider(&ctx).await
                } else {
                    provider
                });
                let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
                let mut worker_ctx = ctx.clone();
                worker_ctx.queue_provider = Some(queue.clone());
                App::connect_workers(&worker_ctx, &queue).await.unwrap();
                yorishiro::workers::embedding_sync::enqueue_for_class(
                    &ctx,
                    EmbeddingSyncArgs {
                        lifecycle_id: None,
                        workspace_id,
                        entity_id,
                        worker_class: WorkerClass::Shared,
                    },
                )
                .await
                .unwrap();
                let running = queue.clone();
                let worker =
                    tokio::spawn(
                        async move { running.run(vec![WorkerTarget::Shared.tag()]).await },
                    );
                let directory = tempfile::tempdir().unwrap();
                let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    async {
                        loop {
                            let dump = queue.dump(directory.path(), None, None).await.unwrap();
                            let jobs: Vec<loco_rs::bgworker::Job> =
                                serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap())
                                    .unwrap();
                            if let Some(job) = jobs.iter().find(|job| {
                                job.data["entity_id"] == entity_id.to_string()
                                    && job.status == JobStatus::Completed
                            }) {
                                let id =
                                    Uuid::parse_str(job.data["lifecycle_id"].as_str().unwrap())
                                        .unwrap();
                                let lifecycle =
                                    Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
                                assert_eq!(
                                    lifecycle.status,
                                    LifecycleStatus::Completed.as_db_str()
                                );
                                assert_eq!(
                                    lifecycle.error.as_deref(),
                                    Some(if disabled {
                                        "embedding provider explicitly disabled"
                                    } else {
                                        "entity has no x-embed content"
                                    })
                                );
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    },
                ))
                .catch_unwind()
                .await;
                App::on_shutdown(&worker_ctx).await;
                queue.shutdown().unwrap();
                let drained =
                    tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
                queue.close().await.unwrap();
                match outcome {
                    Ok(result) => result.expect("configured queue completes the no-op"),
                    Err(panic) => std::panic::resume_unwind(panic),
                }
                drained.unwrap().unwrap().unwrap();
            })
            .await;
        }
    }

    /// A write during embedding cancels the stale delivery; a REST update queues current data.
    #[tokio::test]
    async fn a_write_during_embedding_supersedes_the_old_delivery() {
        use loco_rs::{app::Hooks, bgworker::JobStatus};
        boot_request::<App, _, _>(|request, ctx| async move {
            let (workspace_id, entity_id) = seed(&request, &ctx).await;
            let (provider, _server, entered, release) = vector_provider(768, true).await;
            ctx.shared_store.insert(provider);
            let owner = yorishiro::models::user_users::create_user(
                &ctx.db,
                &crate::requests::fixtures::unique_email("embedding-update"),
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
            let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
            let mut worker_ctx = ctx.clone();
            worker_ctx.queue_provider = Some(queue.clone());
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            yorishiro::workers::embedding_sync::enqueue_for_class(
                &ctx,
                EmbeddingSyncArgs {
                    lifecycle_id: None,
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                },
            )
            .await
            .unwrap();
            let running = queue.clone();
            let worker =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            let directory = tempfile::tempdir().unwrap();
            let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                async {
                    entered.notified().await;
                    entity_entities::update(
                        &ctx.db,
                        workspace_id,
                        entity_entities::UpdateEntityInput {
                            id: entity_id,
                            data: serde_json::json!({"title":"changed while embedding"}),
                            updated_by: None,
                        },
                    )
                    .await
                    .unwrap();
                    release.add_permits(1);
                    let stale_id = loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        if let Some(job) = jobs.iter().find(|job| {
                            job.data["entity_id"] == entity_id.to_string()
                                && job.status == JobStatus::Completed
                        }) {
                            let id = Uuid::parse_str(job.data["lifecycle_id"].as_str().unwrap())
                                .unwrap();
                            let lifecycle =
                                Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
                            assert_eq!(lifecycle.status, LifecycleStatus::Cancelled.as_db_str());
                            assert_eq!(
                                vector_rows(&ctx, entity_id).await,
                                0,
                                "the stale vector must not be stored"
                            );
                            break id;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    };
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
                    entered.notified().await;
                    release.add_permits(1);
                    loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        if let Some(job) = jobs.iter().find(|job| {
                            job.data["entity_id"] == entity_id.to_string()
                                && job.data["lifecycle_id"] != stale_id.to_string()
                                && job.status == JobStatus::Completed
                        }) {
                            let id = Uuid::parse_str(job.data["lifecycle_id"].as_str().unwrap())
                                .unwrap();
                            let lifecycle =
                                Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
                            assert_eq!(lifecycle.status, LifecycleStatus::Completed.as_db_str());
                            assert_eq!(vector_rows(&ctx, entity_id).await, 1);
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                },
            ))
            .catch_unwind()
            .await;
            release.add_permits(2);
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
            queue.close().await.unwrap();
            match outcome {
                Ok(result) => result
                    .expect("configured queue supersedes stale data and completes the REST update"),
                Err(panic) => std::panic::resume_unwind(panic),
            }
            drained.unwrap().unwrap().unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn a_malformed_payload_fails_the_provider_job_without_requeue() {
        use loco_rs::{app::Hooks, bgworker::JobStatus};
        boot_request::<App, _, _>(|_request, ctx| async move {
            let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
            let mut worker_ctx = ctx.clone();
            worker_ctx.queue_provider = Some(queue.clone());
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            let mut owned = Vec::new();
            let mut payloads = Vec::new();
            // Inject invalid persisted deliveries at the queue boundary, bypassing typed enqueue validation.
            for payload in [
                serde_json::json!({"workspace_id": "not-a-uuid", "entity_id": Uuid::now_v7(), "worker_class": "shared"}),
                serde_json::json!({"workspace_id": Uuid::now_v7(), "entity_id": Uuid::now_v7(), "worker_class": "bogus"}),
                serde_json::json!({"workspace_id": Uuid::now_v7(), "worker_class": "shared"}),
            ] {
                payloads.push(payload.clone());
                owned.push(queue.enqueue(
                    EmbeddingSyncWorker::class_name(),
                    Some(WorkerTarget::Shared.queue()),
                    payload,
                    Some(vec![WorkerTarget::Shared.tag()]),
                    Some(100),
                ).await.expect("persist malformed delivery").expect("provider job ID"));
            }
            let running = queue.clone();
            let worker = tokio::spawn(async move {
                running.run(vec![WorkerTarget::Shared.tag()]).await
            });
            let observation = BackendObservation(queue.clone());
            let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
                std::time::Duration::from_secs(10), async {
                    loop {
                        if observation.owned_count(&owned, Some(vec![JobStatus::Failed])).await == 3 {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                },
            )).catch_unwind().await;
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
            let final_statuses = observation.owned_statuses(&owned).await;
            let directory = tempfile::tempdir().unwrap();
            let dump = queue.dump(directory.path(), None, None).await.unwrap();
            let jobs: Vec<loco_rs::bgworker::Job> =
                serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
            queue.close().await.unwrap();
            match outcome {
                Ok(result) => result.expect("malformed deliveries fail in the configured provider"),
                Err(panic) => std::panic::resume_unwind(panic),
            }
            drained.unwrap().unwrap().unwrap();
            assert_eq!(final_statuses.len(), 3);
            assert_eq!(jobs.iter().filter(|job| payloads.iter().any(|payload| {
                payload.as_object().unwrap().iter().all(|(key, value)| job.data.get(key) == Some(value))
            })).count(), 3,
                "no replacement delivery may be enqueued");
            assert!(final_statuses.iter().all(|(_, status)| *status == JobStatus::Failed));
        }).await;
    }

    /// An entity deleted while it is being embedded is the one zero-row outcome that is not a conflict: there is nothing left to embed.
    /// The job finishes without error and without storing a vector.
    #[tokio::test]
    async fn an_entity_deleted_during_embedding_is_skipped_without_a_vector() {
        use loco_rs::{app::Hooks, bgworker::JobStatus};
        boot_request::<App, _, _>(|request, ctx| async move {
            let (workspace_id, entity_id) = seed(&request, &ctx).await;
            let (provider, _server, entered, release) = vector_provider(768, true).await;
            ctx.shared_store.insert(provider);

            let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
            let mut worker_ctx = ctx.clone();
            worker_ctx.queue_provider = Some(queue.clone());
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            yorishiro::workers::embedding_sync::enqueue_for_class(
                &ctx,
                EmbeddingSyncArgs {
                    lifecycle_id: None,
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                },
            )
            .await
            .unwrap();
            let running = queue.clone();
            let worker =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            let directory = tempfile::tempdir().unwrap();
            let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                async {
                    tokio::join!(
                        async {
                            loop {
                                let dump = queue.dump(directory.path(), None, None).await.unwrap();
                                let jobs: Vec<loco_rs::bgworker::Job> =
                                    serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap())
                                        .unwrap();
                                if let Some(job) = jobs.iter().find(|job| {
                                    job.data["entity_id"] == entity_id.to_string()
                                        && job.status == JobStatus::Completed
                                }) {
                                    let id =
                                        Uuid::parse_str(job.data["lifecycle_id"].as_str().unwrap())
                                            .unwrap();
                                    let lifecycle =
                                        Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
                                    assert_eq!(
                                        lifecycle.status,
                                        LifecycleStatus::Completed.as_db_str()
                                    );
                                    assert_eq!(vector_rows(&ctx, entity_id).await, 0);
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            }
                        },
                        async {
                            entered.notified().await;
                            entity_entities::delete(&ctx.db, workspace_id, entity_id)
                                .await
                                .unwrap();
                            release.add_permits(1);
                        }
                    )
                },
            ))
            .catch_unwind()
            .await;
            release.add_permits(1);
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
            queue.close().await.unwrap();
            match outcome {
                Ok(result) => {
                    result.expect("configured queue skips the deleted entity");
                }
                Err(panic) => std::panic::resume_unwind(panic),
            }
            drained.unwrap().unwrap().unwrap();
        })
        .await;
    }

    /// Structural failures are recorded as terminal lifecycle failures rather than completed jobs.
    #[tokio::test]
    async fn a_live_dimension_failure_is_not_marked_completed() {
        use loco_rs::{app::Hooks, bgworker::JobStatus};
        boot_request::<App, _, _>(|request, ctx| async move {
            let (workspace_id, entity_id) = seed(&request, &ctx).await;
            let record = entity_entities::get(&ctx.db, workspace_id, entity_id)
                .await
                .expect("read entity");
            let (original_provider, _original_server, _, _) = vector_provider(768, false).await;
            yorishiro::models::entity_embeddings::sync_embedding_for_record(
                &ctx.db,
                workspace_id,
                &record,
                original_provider.as_ref(),
            )
            .await
            .expect("persist original vector and workspace stamp");
            let (provider, _server, _, _) = vector_provider(1024, false).await;
            ctx.shared_store.insert(original_provider.clone());

            let queue = crate::requests::query_worker::shared_target_queue(&ctx).await;
            let mut worker_ctx = ctx.clone();
            worker_ctx.queue_provider = Some(queue.clone());
            App::connect_workers(&worker_ctx, &queue).await.unwrap();
            assert!(
                !Entity::has_active(&ctx.db, "startup_reindex", workspace_id)
                    .await
                    .unwrap()
            );
            // Switch providers after connect so the startup scan sees the stamped width.
            ctx.shared_store.insert(provider);
            yorishiro::workers::embedding_sync::enqueue_for_class(
                &ctx,
                EmbeddingSyncArgs {
                    lifecycle_id: None,
                    workspace_id,
                    entity_id,
                    worker_class: WorkerClass::Shared,
                },
            )
            .await
            .unwrap();
            let running = queue.clone();
            let worker =
                tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
            let directory = tempfile::tempdir().unwrap();
            let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                async {
                    loop {
                        let dump = queue.dump(directory.path(), None, None).await.unwrap();
                        let jobs: Vec<loco_rs::bgworker::Job> =
                            serde_yaml::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
                        if let Some(job) = jobs.iter().find(|job| {
                            job.data["entity_id"] == entity_id.to_string()
                                && job.status == JobStatus::Failed
                        }) {
                            let id = Uuid::parse_str(job.data["lifecycle_id"].as_str().unwrap())
                                .unwrap();
                            let lifecycle =
                                Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
                            assert_eq!(lifecycle.status, LifecycleStatus::Failed.as_db_str());
                            assert!(lifecycle.error.is_some());
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                },
            ))
            .catch_unwind()
            .await;
            App::on_shutdown(&worker_ctx).await;
            queue.shutdown().unwrap();
            let drained = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
            queue.close().await.unwrap();
            match outcome {
                Ok(result) => result.expect("configured queue records dimension failure"),
                Err(panic) => std::panic::resume_unwind(panic),
            }
            drained.unwrap().unwrap().unwrap();
        })
        .await;
    }
}
