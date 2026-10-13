use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

/// Stamps the workspace only when it does not have a model of its own yet.
pub(super) async fn stamp_workspace_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    model: String,
    dimensions: i32,
) -> Result<(), YorishiroError> {
    crate::models::workspace_workspaces::stamp_embedding_if_missing(
        conn,
        workspace_id,
        model,
        dimensions,
    )
    .await
}

/// Restamps the workspace after every candidate has been reindexed successfully.
pub(super) async fn restamp_workspace_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    model: String,
    dimensions: i32,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::workspace_workspaces::{Column, Entity};
    use sea_orm::sea_query::Expr;

    Entity::update_many()
        .col_expr(Column::EmbeddingModel, Expr::value(model))
        .col_expr(Column::EmbeddingDimensions, Expr::value(dimensions))
        .col_expr(Column::EmbeddingReindexing, Expr::value(false))
        .filter(Column::Id.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
