use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

/// Stamps the workspace only when it does not have a model of its own yet.
pub(super) async fn stamp_workspace_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    model: String,
    dimensions: i32,
) -> Result<(), YorishiroError> {
    crate::models::identity::workspace_workspaces::stamp_embedding_if_missing(
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
    let mut active = crate::models::identity::workspace_workspaces::ActiveModel {
        id: ActiveValue::Unchanged(workspace_id),
        ..Default::default()
    };
    active.embedding_model = ActiveValue::Set(Some(model));
    active.embedding_dimensions = ActiveValue::Set(Some(dimensions));
    active.update(conn).await.internal()?;
    Ok(())
}
