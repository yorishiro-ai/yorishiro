use loco_rs::prelude::*;
use loco_rs::task::Vars;

pub(crate) struct EmbeddingIndexMaintenance;

#[async_trait]
impl Task for EmbeddingIndexMaintenance {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "embedding_index_maintenance".to_string(),
            detail: "Promotes workspace HNSW indexes and retires obsolete ones".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &Vars) -> Result<()> {
        crate::models::entity_embeddings::maintain_workspace_indexes(&ctx.db).await?;
        Ok(())
    }
}
