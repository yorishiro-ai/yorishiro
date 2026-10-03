use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::YorishiroError;

/// The resolved embedding chain for a workspace.
#[derive(Clone)]
pub(crate) struct ResolvedEmbedding {
    pub(crate) workspace_model: Option<String>,
    pub(crate) workspace_dimensions: Option<i32>,
    pub(crate) tenant_model: Option<String>,
    pub(crate) tenant_dimensions: Option<i32>,
    pub(crate) deployment_dimensions: usize,
}

/// Resolves the workspace, tenant, and deployment embedding settings.
pub(crate) async fn resolve_embedding_chain(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    deployment_dimensions: usize,
) -> Result<ResolvedEmbedding, YorishiroError> {
    let row = crate::models::workspace_workspaces::embedding_chain(conn, workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))?;

    Ok(ResolvedEmbedding {
        workspace_model: row.embedding_model,
        workspace_dimensions: row.embedding_dimensions,
        tenant_model: row.tenant_model,
        tenant_dimensions: row.tenant_dimensions,
        deployment_dimensions,
    })
}
