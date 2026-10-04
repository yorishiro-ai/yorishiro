//! Control-plane use cases that compose more than one tenancy table.

use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::_entities::workspace_workspaces;
use crate::models::workspace_workspaces::WorkspaceSummary;

mod orchestration;

/// Lists every workspace a user can log into across all tenant memberships.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    orchestration::list_workspaces_for_user(conn, user_id).await
}

/// Creates a workspace while enforcing its tenant-owned workspace limit.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_workspace(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    orchestration::create_workspace(conn, tenant_id, name, max_entities, schema_id).await
}
