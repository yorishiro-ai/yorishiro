use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

/// Stamps the workspace only when it does not have a model of its own yet.
pub(super) async fn stamp_workspace_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    model: String,
    dimensions: i32,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;

    crate::models::workspace_workspaces::Entity::update_many()
        .col_expr(Column::EmbeddingModel, Expr::value(model))
        .col_expr(Column::EmbeddingDimensions, Expr::value(dimensions))
        .filter(Column::Id.eq(workspace_id))
        .filter(Column::EmbeddingModel.is_null())
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Restamps the workspace after every candidate has been reindexed successfully.
pub(super) async fn restamp_workspace_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    model: String,
    dimensions: i32,
) -> Result<(), YorishiroError> {
    let mut active = crate::models::workspace_workspaces::ActiveModel {
        id: ActiveValue::Unchanged(workspace_id),
        ..Default::default()
    };
    active.embedding_model = ActiveValue::Set(Some(model));
    active.embedding_dimensions = ActiveValue::Set(Some(dimensions));
    active.update(conn).await.internal()?;
    Ok(())
}
