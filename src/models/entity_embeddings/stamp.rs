use sea_orm::{ConnectionTrait, Statement};
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
    let backend = conn.get_database_backend();
    conn.execute_raw(Statement::from_sql_and_values(
        backend,
        "UPDATE workspace_workspaces SET embedding_model = $1, embedding_dimensions = $2, \
         embedding_reindexing = FALSE WHERE id = $3",
        [model.into(), dimensions.into(), workspace_id.into()],
    ))
    .await
    .internal()?;
    Ok(())
}
