//! This crate's `WorkspaceEmbeddingResolver`: a workspace with its own row in `identity_workspace_embedding_keys` uses that provider instead of the deployment default.

use std::sync::Arc;

use crate::error::YorishiroError;
use crate::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider, WorkspaceEmbeddingResolver,
};
use async_trait::async_trait;
use uuid::Uuid;

use crate::ee::models::embedding_keys;

pub struct EmbeddingKeyResolver;

#[async_trait]
impl WorkspaceEmbeddingResolver for EmbeddingKeyResolver {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<Arc<dyn EmbeddingProvider>>, YorishiroError> {
        let Some(config) = embedding_keys::get(conn, workspace_id).await? else {
            return Ok(None);
        };

        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: config.base_url,
            api_key: config.api_key,
            model: config.model,
            dimensions: config.dimensions as usize,
            send_dimensions_param: config.send_dimensions_param,
        });
        Ok(Some(Arc::new(provider)))
    }
}
