use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loco_rs::bgworker::BackgroundWorker;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use uuid::Uuid;

use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::embedding::{EmbeddingProvider, WorkspaceEmbeddingResolver};
use yorishiro::workers::embedding_sync::WorkerClass;
use yorishiro::workers::reindex::{ReindexArgs, ReindexWorkerShared};

struct Provider {
    model: &'static str,
    width: usize,
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
        Ok(texts.iter().map(|_| vec![1.0; self.width]).collect())
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
        embedding_dimensions: Set(Some(768)),
        embedding_model: Set(Some("default-model".into())),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .expect("workspace");
    let calls = Arc::new(Mutex::new(0));
    let provider = Arc::new(Provider {
        model: "workspace-model",
        width: 768,
        calls: calls.clone(),
    });
    ctx.shared_store
        .insert(Arc::new(Resolver(provider.clone())) as Arc<dyn WorkspaceEmbeddingResolver>);
    ctx.shared_store
        .insert(provider as Arc<dyn EmbeddingProvider>);
    let args = ReindexArgs {
        lifecycle_id: None,
        workspace_id: workspace.id,
        worker_class: WorkerClass::Shared,
        startup: false,
    };
    let encoded = serde_json::to_string(&args).expect("serialize args");
    assert!(!encoded.contains("key") && !encoded.contains("secret"));
    ReindexWorkerShared::build(&ctx)
        .perform(args)
        .await
        .expect("reindex");
    let row = workspace_workspaces::Entity::find_by_id(workspace.id)
        .one(&ctx.db)
        .await
        .expect("workspace")
        .expect("row");
    assert_eq!(row.embedding_model.as_deref(), Some("workspace-model"));
    assert_eq!(row.embedding_dimensions, Some(768));
    assert!(*calls.lock().unwrap() > 0);
}

#[tokio::test]
async fn absent_and_disabled_providers_are_terminal_worker_errors() {
    for provider in [
        Arc::new(Missing) as Arc<dyn WorkspaceEmbeddingResolver>,
        Arc::new(Resolver(Arc::new(Disabled))) as Arc<dyn WorkspaceEmbeddingResolver>,
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
        let result = ReindexWorkerShared::build(&ctx)
            .perform(ReindexArgs {
                lifecycle_id: None,
                workspace_id: workspace.id,
                worker_class: WorkerClass::Shared,
                startup: false,
            })
            .await;
        assert!(result.is_err());
    }
}
