//! Workspace embedding resolution installed over the base default.

use std::sync::Arc;

use loco_rs::app::AppContext;

pub(crate) fn install(ctx: &AppContext) {
    ctx.shared_store.insert(Arc::new(
        crate::edition::ee::models::workspace_embedding_keys::EmbeddingKeyResolver,
    )
        as Arc<dyn crate::services::embedding::WorkspaceEmbeddingResolver>);
}
