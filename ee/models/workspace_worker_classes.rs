//! A workspace's own worker-class assignment: which compute its embedding-sync jobs run on.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `workspace_llm_keys`/`workspace_embedding_keys`.
//!
//! A workspace with no row here stays `WorkerClass::Shared` (`WorkerClassResolver::resolve` returns `None`); this module never falls back on its own, so the caller (`WorkerClassAssignmentResolver`) decides that.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_worker_classes::{ActiveModel, Column, Entity};
use crate::workers::embedding_sync::{WorkerClass, WorkerClassResolver};
use async_trait::async_trait;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

/// Resolves the explicit assignment from this table, leaving the shared fallback to the caller.
pub struct WorkerClassAssignmentResolver;

#[async_trait]
impl WorkerClassResolver for WorkerClassAssignmentResolver {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError> {
        get(conn, workspace_id).await
    }
}

/// What a workspace has configured, for an endpoint to report.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WorkerClassAssignment {
    pub(crate) worker_class: WorkerClass,
}

/// Stores or replaces a workspace's own worker-class assignment.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn set(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    worker_class: WorkerClass,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        worker_class: ActiveValue::Set(worker_class.as_db_str().to_string()),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::WorkspaceId)
                .update_columns([Column::WorkerClass, Column::UpdatedAt])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Removes a workspace's own assignment.
/// It falls back to `WorkerClass::Shared` afterward.
pub(crate) async fn clear(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report.
pub(crate) async fn describe(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<WorkerClassAssignment>, YorishiroError> {
    get(conn, workspace_id)
        .await
        .map(|found| found.map(|worker_class| WorkerClassAssignment { worker_class }))
}

/// The assignment itself, as `WorkerClassAssignmentResolver` reads it to route a queued job.
/// `None` means the workspace has configured none, which the resolver reads as "fall back to `WorkerClass::Shared`".
pub(crate) async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<WorkerClass>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?;

    let Some(row) = row else {
        return Ok(None);
    };
    WorkerClass::from_db_str(&row.worker_class)
        .map(Some)
        .ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown worker class in database: {}",
                row.worker_class
            ))
        })
}
