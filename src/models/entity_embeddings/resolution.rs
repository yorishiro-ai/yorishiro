use crate::error::OptionExt;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

/// The resolved embedding settings for a workspace.
#[derive(Clone)]
pub(crate) struct ResolvedEmbedding {
    pub(crate) workspace_model: Option<String>,
    pub(crate) workspace_dimensions: Option<i32>,
    pub(crate) deployment_dimensions: usize,
}

impl ResolvedEmbedding {
    pub(super) fn dimensions(&self) -> Result<usize, YorishiroError> {
        self.workspace_dimensions.map_or_else(
            || {
                Ok(if i32::try_from(self.deployment_dimensions).is_ok() {
                    self.deployment_dimensions
                } else {
                    crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS
                })
            },
            |dimensions| usize::try_from(dimensions).internal(),
        )
    }
}

/// Resolves the workspace and deployment embedding settings.
pub(crate) async fn resolve_embedding_chain(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    deployment_dimensions: usize,
) -> Result<ResolvedEmbedding, YorishiroError> {
    let row = crate::models::workspace_workspaces::embedding_chain(conn, workspace_id)
        .await?
        .or_not_found(format!("workspace {workspace_id} not found"))?;

    Ok(ResolvedEmbedding {
        workspace_model: row.embedding_model,
        workspace_dimensions: row.embedding_dimensions,
        deployment_dimensions,
    })
}
