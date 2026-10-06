use super::boot_request;
use async_trait::async_trait;
use axum::http::StatusCode;
use serial_test::serial;
use std::sync::{Arc, Mutex};
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{api_keys, tenant_tenants, workspace_workspaces};
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::embedding::{EmbedKind, EmbeddingProvider};

struct Setup {
    read_key: String,
    tenant_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("search-req-test".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let owner = yorishiro::models::user_users::create_user(
        &ctx.db,
        "owner@example.com",
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create owner");
    yorishiro::models::tenant_memberships::add_member(
        &ctx.db,
        tenant.id,
        owner.id,
        MembershipRole::Owner,
    )
    .await
    .expect("add owner");
    let read_key = api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Read,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue read key")
    .plaintext;
    Setup {
        read_key,
        tenant_id: tenant.id,
        workspace_id: workspace.id,
    }
}

struct RecordingProvider {
    kinds: Arc<Mutex<Vec<EmbedKind>>>,
}

#[async_trait]
impl EmbeddingProvider for RecordingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> String {
        "recording-provider".into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts.iter().map(|_| vec![0.0; 768]).collect())
    }

    async fn embed_as(&self, kind: EmbedKind, _text: &str) -> Result<Vec<f32>, YorishiroError> {
        self.kinds.lock().unwrap().push(kind);
        Ok(vec![1.0; 768])
    }
}

#[tokio::test]
async fn search_embeds_the_query_synchronously_as_query_kind() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup { read_key, .. } = setup(&ctx).await;
        let kinds = Arc::new(Mutex::new(Vec::new()));
        ctx.shared_store.insert(Arc::new(RecordingProvider {
            kinds: kinds.clone(),
        }) as Arc<dyn EmbeddingProvider>);

        let response = request
            .get("/api/search?query_text=hello")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;

        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{}",
            response.text()
        );
        assert!(
            kinds.lock().unwrap().as_slice() == [EmbedKind::Query],
            "search must make exactly one query embedding call"
        );
    })
    .await;
}

/// Entity create, the real worker, then `GET /api/search` through the tenant-scoped transaction: the whole semantic path on whichever backend `DATABASE_URL` names.
/// The worker embeds as `Document` and the server embeds the query as `Query`, both through the same provider.
#[tokio::test]
async fn search_finds_an_entity_embedded_by_the_worker() {
    use loco_rs::bgworker::{BackgroundWorker, sqlt};
    use loco_rs::config::SqliteQueueConfig;
    use sea_orm::{EntityTrait, FromQueryResult, Statement};
    use yorishiro::models::queue_job_lifecycles::{
        Enqueue, Entity as LifecycleEntity, LifecycleStatus,
    };
    use yorishiro::models::{entity_entities, schema_schemas};
    use yorishiro::workers::embedding_sync::{
        EmbeddingSyncArgs, EmbeddingSyncWorkerShared, WorkerClass,
    };

    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }

    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup {
            read_key,
            tenant_id,
            workspace_id,
        } = setup(&ctx).await;
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
                data: serde_json::json!({ "title": "quarterly roadmap review" }),
            },
            None,
        )
        .await
        .expect("create entity");

        let kinds = Arc::new(Mutex::new(Vec::new()));
        ctx.shared_store.insert(Arc::new(RecordingProvider {
            kinds: kinds.clone(),
        }) as Arc<dyn EmbeddingProvider>);

        let queue_dir = tempfile::tempdir().expect("queue tempdir");
        let queue_uri = format!(
            "sqlite://{}?mode=rwc",
            queue_dir.path().join("queue.sqlite3").display()
        );
        let queue = std::sync::Arc::new(
            sqlt::create_provider(&SqliteQueueConfig {
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
            .expect("SQLite queue"),
        );
        queue.setup().await.expect("set up queue");
        let ctx = ctx.into_builder().queue_provider(queue.clone()).build();
        let lifecycle_id = uuid::Uuid::now_v7();
        LifecycleEntity::record_enqueue(
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
        let args = EmbeddingSyncArgs {
            lifecycle_id: Some(lifecycle_id),
            workspace_id,
            entity_id: entity.id,
            worker_class: WorkerClass::Shared,
        };
        EmbeddingSyncWorkerShared::perform_later_with_priority(&ctx, args, Some(100))
            .await
            .expect("dispatch embedding worker");
        let queue_pool = sqlt::SqlitePool::connect(&queue_uri)
            .await
            .expect("connect to queue");
        let queued = sqlt::get_jobs(&queue_pool, None, None)
            .await
            .expect("read queued jobs");
        assert_eq!(queued.len(), 1, "job must reach the queue: {queued:?}");
        assert_eq!(queued[0].name, "EmbeddingSyncWorkerShared");

        queue
            .register(EmbeddingSyncWorkerShared::build(&ctx))
            .await
            .expect("register embedding worker");
        let running = queue.clone();
        let worker =
            tokio::spawn(async move { running.run(vec![WorkerClass::Shared.tag().into()]).await });
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let row = LifecycleEntity::find_by_id(lifecycle_id)
                    .one(&ctx.db)
                    .await
                    .expect("read lifecycle")
                    .expect("lifecycle exists");
                if row.status == LifecycleStatus::Completed.as_db_str() {
                    break row.status;
                }
                assert_ne!(row.status, LifecycleStatus::Failed.as_db_str());
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("worker must dequeue and complete the lifecycle");
        assert_eq!(status, LifecycleStatus::Completed.as_db_str());
        let row_count = Count::find_by_statement(Statement::from_sql_and_values(
            ctx.db.get_database_backend(),
            "SELECT COUNT(*) AS n FROM entity_embeddings_768 WHERE entity_id = $1",
            [entity.id.into()],
        ))
        .one(&ctx.db)
        .await
        .expect("count stored vectors")
        .expect("count row")
        .n;
        assert_eq!(
            row_count, 1,
            "worker completion must persist the document vector"
        );
        let _ = queue.shutdown();
        queue_pool.close().await;
        let _ = worker.await;

        let response = request
            .get("/api/search?query_text=roadmap")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{}",
            response.text()
        );
        let hits: serde_json::Value = response.json();
        assert_eq!(hits.as_array().map(Vec::len), Some(1), "hits: {hits}");
        assert_eq!(hits[0]["entity"]["id"], entity.id.to_string());
        assert!(
            hits[0]["distance"].is_number(),
            "the hit must come from the stored vector, not the text fallback: {hits}"
        );
        assert!(
            kinds.lock().unwrap().as_slice() == [EmbedKind::Document, EmbedKind::Query],
            "the worker embeds as Document, the server as Query"
        );
    })
    .await;
}

/// `YORISHIRO_EMBEDDING_PROVIDER=none` forces `build_embedding_provider` to return `UnconfiguredEmbeddingProvider` regardless of cached model files, so any actual search attempt surfaces as 502, not a panic or a silent empty result.
/// Search fails loudly and namedly rather than the boot process itself failing for every deployment that hasn't configured embeddings yet.
#[tokio::test]
#[serial(process_environment)]
async fn search_with_no_embedding_provider_configured_returns_502() {
    // Force unconfigured provider even if model files exist in cache: the test asserts on the
    // provider-missing path, not on the local provider succeeding.
    let guard = crate::EnvGuard::capture(&["YORISHIRO_EMBEDDING_PROVIDER"]);
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");

    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup { read_key, .. } = setup(&ctx).await;

        let response = request
            .get("/api/search?query_text=hello")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            502,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// The auth check (`Verified`) runs before the embedding call: no key at all must be rejected with 401, not 502.
/// `Read` is the lowest scope, so there's no "too-low scope" case to test against a read-gated endpoint beyond this.
#[tokio::test]
#[serial(process_environment)]
async fn search_requires_authentication() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/search?query_text=hello").await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    })
    .await;
}
