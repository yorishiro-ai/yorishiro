use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loco_rs::bgworker::BackgroundWorker;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, EntityTrait, Statement};
use uuid::Uuid;

use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::{entity_entities, entity_entities::CreateEntityInput, schema_schemas};
use yorishiro::services::embedding::{EmbeddingProvider, WorkspaceEmbeddingResolver};
use yorishiro::workers::embedding_sync::WorkerClass;
use yorishiro::workers::reindex::{ReindexArgs, ReindexWorker};
use yorishiro::workers::target::WorkerTarget;

struct Provider {
    model: &'static str,
    width: usize,
    value: f32,
    calls: Arc<Mutex<usize>>,
}

#[async_trait]
impl EmbeddingProvider for Provider {
    fn dimensions(&self) -> usize {
        self.width
    }
    fn model_name(&self) -> String {
        self.model.into()
    }
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, yorishiro::error::YorishiroError> {
        *self.calls.lock().unwrap() += texts.len().max(1);
        Ok(texts.iter().map(|_| vec![self.value; self.width]).collect())
    }
}

struct Resolver(Arc<dyn EmbeddingProvider>);

#[async_trait]
impl WorkspaceEmbeddingResolver for Resolver {
    async fn resolve(
        &self,
        _conn: &sea_orm::DatabaseConnection,
        _workspace_id: Uuid,
    ) -> Result<Option<Arc<dyn EmbeddingProvider>>, yorishiro::error::YorishiroError> {
        Ok(Some(self.0.clone()))
    }
}

struct Disabled;

#[async_trait]
impl EmbeddingProvider for Disabled {
    fn availability(&self) -> yorishiro::services::embedding::EmbeddingProviderAvailability {
        yorishiro::services::embedding::EmbeddingProviderAvailability::Disabled
    }
    fn dimensions(&self) -> usize {
        768
    }
    fn model_name(&self) -> String {
        "disabled".into()
    }
    async fn embed_batch(
        &self,
        _texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, yorishiro::error::YorishiroError> {
        unreachable!("disabled provider must be rejected before embedding")
    }
}

struct Missing;

#[async_trait]
impl WorkspaceEmbeddingResolver for Missing {
    async fn resolve(
        &self,
        _conn: &sea_orm::DatabaseConnection,
        _workspace_id: Uuid,
    ) -> Result<Option<Arc<dyn EmbeddingProvider>>, yorishiro::error::YorishiroError> {
        Ok(None)
    }
}

async fn record_delivery(ctx: &loco_rs::app::AppContext, workspace_id: Uuid) -> (Uuid, i64) {
    use yorishiro::models::queue_job_lifecycles::Enqueue;

    let lifecycle_id = Uuid::now_v7();
    yorishiro::models::queue_job_dispatch_outbox::record(
        &ctx.db,
        Enqueue {
            id: lifecycle_id,
            job_name: "reindex_embeddings",
            worker_class: WorkerClass::Shared,
            workspace_id: Some(workspace_id),
            plan: None,
            concurrency_key: None,
            concurrency_limit: None,
        },
        serde_json::json!({ "workspace_id": workspace_id, "worker_class": WorkerClass::Shared, "startup": false }),
        ReindexWorker::class_name(),
        Some(WorkerTarget::Shared.queue()),
        Some(vec![WorkerTarget::Shared.tag()]),
    )
    .await
    .expect("record current-epoch reindex delivery");
    let epoch = yorishiro::models::queue_job_dispatch_outbox::publishing_state(&ctx.db)
        .await
        .expect("read publisher state")
        .epoch;
    (lifecycle_id, epoch)
}

#[tokio::test]
async fn worker_uses_workspace_provider_and_keeps_key_out_of_payload() {
    let ctx = crate::workers::test_context().await;
    Migrator::up(&ctx.db, None).await.expect("migrations");
    let tenant = tenant_tenants::ActiveModel {
        name: Set("ee-reindex".into()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: Set(tenant.id),
        name: Set("main".into()),
        status: Set(WORKSPACE_STATUS_ACTIVE.into()),
        embedding_dimensions: Set(Some(1536)),
        embedding_model: Set(Some("deployment-model".into())),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("workspace");
    let default_calls = Arc::new(Mutex::new(0));
    let workspace_calls = Arc::new(Mutex::new(0));
    let default = Arc::new(Provider {
        model: "deployment-model",
        width: 1536,
        value: 1.0,
        calls: default_calls.clone(),
    });
    let workspace_provider = Arc::new(Provider {
        model: "workspace-model",
        width: 1024,
        value: 2.0,
        calls: workspace_calls.clone(),
    });
    schema_schemas::create_schema(
        &ctx.db,
        tenant.id,
        workspace.id,
        serde_json::from_value(serde_json::json!({"name":"note","entity_types":{"note":{"fields":{"title":{"type":"string","required":true,"x-embed":true}}}}})).expect("definition"),
        None,
        None,
    ).await.expect("schema");
    let entity = entity_entities::create(
        &ctx.db,
        workspace.id,
        CreateEntityInput {
            schema_name: "note".into(),
            entity_type: "note".into(),
            data: serde_json::json!({"title":"workspace provider candidate"}),
        },
        None,
    )
    .await
    .expect("entity");
    ctx.shared_store
        .insert(Arc::new(Resolver(workspace_provider)) as Arc<dyn WorkspaceEmbeddingResolver>);
    ctx.shared_store
        .insert(default as Arc<dyn EmbeddingProvider>);
    let (lifecycle_id, epoch) = record_delivery(&ctx, workspace.id).await;
    let args = ReindexArgs {
        lifecycle_id: Some(lifecycle_id),
        dispatch_id: Some(lifecycle_id),
        publishing_epoch: Some(epoch),
        workspace_id: workspace.id,
        worker_class: WorkerClass::Shared,
        startup: false,
    };
    let encoded = serde_json::to_string(&args).expect("serialize args");
    assert!(!encoded.contains("key") && !encoded.contains("secret"));
    ReindexWorker::build(&ctx)
        .perform(args)
        .await
        .expect("reindex");
    let row = workspace_workspaces::Entity::find_by_id(workspace.id)
        .one(&ctx.db)
        .await
        .expect("workspace")
        .expect("row");
    assert_eq!(row.embedding_model.as_deref(), Some("workspace-model"));
    assert_eq!(row.embedding_dimensions, Some(1024));
    assert!(*workspace_calls.lock().unwrap() >= 2);
    assert_eq!(*default_calls.lock().unwrap(), 0);
    let bytes: i64 = ctx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT length(embedding) FROM entity_embeddings_1024 WHERE entity_id = ?",
            [entity.id.into()],
        ))
        .await
        .expect("embedding row")
        .expect("embedding exists")
        .try_get_by_index(0)
        .expect("embedding length");
    assert_eq!(bytes, 1024 * 4);
}

#[tokio::test]
async fn absent_and_disabled_providers_are_terminal_worker_errors() {
    for (provider, expected_error) in [
        (
            Arc::new(Missing) as Arc<dyn WorkspaceEmbeddingResolver>,
            "embedding provider absent",
        ),
        (
            Arc::new(Resolver(Arc::new(Disabled))) as Arc<dyn WorkspaceEmbeddingResolver>,
            "embedding provider disabled",
        ),
    ] {
        let ctx = crate::workers::test_context().await;
        Migrator::up(&ctx.db, None).await.expect("migrations");
        let tenant = tenant_tenants::ActiveModel {
            name: Set("ee-terminal".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("tenant");
        let workspace = workspace_workspaces::ActiveModel {
            tenant_id: Set(tenant.id),
            name: Set("main".into()),
            status: Set(WORKSPACE_STATUS_ACTIVE.into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("workspace");
        ctx.shared_store.insert(provider);
        let (lifecycle_id, epoch) = record_delivery(&ctx, workspace.id).await;
        let result = ReindexWorker::build(&ctx)
            .perform(ReindexArgs {
                lifecycle_id: Some(lifecycle_id),
                dispatch_id: Some(lifecycle_id),
                publishing_epoch: Some(epoch),
                workspace_id: workspace.id,
                worker_class: WorkerClass::Shared,
                startup: false,
            })
            .await;
        result.expect("terminal worker errors are recorded on the lifecycle");
        let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(lifecycle_id)
            .one(&ctx.db)
            .await
            .expect("read terminal lifecycle")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.status, "failed");
        assert!(
            lifecycle
                .error
                .as_deref()
                .is_some_and(|error| error.contains(expected_error)),
            "error={:?}, expected substring={expected_error}",
            lifecycle.error
        );
    }
}
