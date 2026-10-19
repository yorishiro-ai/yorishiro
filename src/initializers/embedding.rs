//! Resolver and concurrency setup, with expensive providers loaded only by worker processes.

use loco_rs::{Result, app::AppContext};

pub(crate) fn install(ctx: &AppContext, settings: &crate::data::settings::Settings) {
    ctx.shared_store
        .insert(crate::services::embedding::default_embedding_resolver());
    ctx.shared_store
        .insert(std::sync::Arc::new(tokio::sync::Semaphore::new(
            settings.embedding.provider_concurrency,
        )));
}

pub(crate) async fn install_provider(ctx: &AppContext) -> Result<()> {
    if ctx
        .shared_store
        .get::<std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>>()
        .is_some()
    {
        return Ok(());
    }
    let settings = ctx
        .shared_store
        .get::<crate::data::settings::Settings>()
        .ok_or_else(|| loco_rs::Error::Message("application settings missing".into()))?;
    let provider = crate::services::embedding::build_embedding_provider(&settings)
        .await
        .map_err(|error| {
            loco_rs::Error::Message(format!("failed to build embedding provider: {error}"))
        })?;
    ctx.shared_store.insert(provider);
    Ok(())
}
