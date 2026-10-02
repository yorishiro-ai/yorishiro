use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

use super::{EmbeddingChainRow, ResolvedEmbedding};

pub(super) async fn resolve_embedding_chain(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    licenced: bool,
) -> Result<ResolvedEmbedding, YorishiroError> {
    use crate::models::_entities::tenant_tenants::Column as TenantColumn;
    use crate::models::_entities::workspace_workspaces::Column;

    let row = crate::models::identity::workspace_workspaces::Entity::find()
        .select_only()
        .column(Column::EmbeddingModel)
        .column(Column::EmbeddingDimensions)
        .column_as(TenantColumn::EmbeddingModel, "tenant_model")
        .column_as(TenantColumn::EmbeddingDimensions, "tenant_dimensions")
        .left_join(crate::models::identity::tenant_tenants::Entity)
        .filter(Column::Id.eq(workspace_id))
        .into_model::<EmbeddingChainRow>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))?;

    let deployment_dimensions: usize = std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS.to_string())
        .parse()
        .unwrap_or(crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS);

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
