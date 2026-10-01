use async_trait::async_trait;
use loco_rs::app::AppContext;
#[cfg(feature = "test-support")]
use std::sync::Arc;

use super::embedding_sync::EmbeddingSyncArgs;
use super::reindex::ReindexArgs;

/// The typed dispatch seam used by the selected application enqueue paths.
#[async_trait]
pub(crate) trait EmbeddingSyncDispatcher: Send + Sync {
    async fn dispatch(&self, ctx: &AppContext, args: EmbeddingSyncArgs) -> loco_rs::Result<String>;
}

#[async_trait]
pub trait ReindexDispatcher: Send + Sync {
    async fn dispatch(&self, ctx: &AppContext, args: ReindexArgs) -> loco_rs::Result<String>;
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
