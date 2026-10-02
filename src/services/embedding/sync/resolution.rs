use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::YorishiroError;

use super::ResolvedEmbedding;

pub(super) async fn resolve_embedding_chain(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    licenced: bool,
    deployment_dimensions: usize,
) -> Result<ResolvedEmbedding, YorishiroError> {
    let row = crate::models::identity::workspace_workspaces::embedding_chain(conn, workspace_id)
        .await?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))?;

    Ok(ResolvedEmbedding {
        workspace_model: row.embedding_model,
        workspace_dimensions: if licenced {
            row.embedding_dimensions
        } else {
            None
        },
        tenant_model: if licenced { row.tenant_model } else { None },
        tenant_dimensions: if licenced {
            row.tenant_dimensions
        } else {
            None
        },
        deployment_dimensions,
    })
}
