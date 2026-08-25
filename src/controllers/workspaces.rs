use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::extractors::{Authorized, ReadScope, embedding_provider};
use crate::controllers::members::require_tenant_admin;
use crate::error::YorishiroError;
use crate::models::_entities::identity_workspaces;
use crate::models::tenancy;
use crate::models::{content_entities, content_relations, content_schemas};

/// Fetches a workspace and confirms it belongs to `tenant_id`, so a caller can never probe or act on another tenant's workspace by guessing its id.
/// `identity_workspaces` has no RLS of its own (it's read through `ctx.db`, the migration-role connection), so this check is the only thing enforcing that boundary for these handlers.
async fn get_workspace_in_tenant(
    ctx: &AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<identity_workspaces::Model, ApiError> {
    let workspace = tenancy::get_workspace(&ctx.db, workspace_id).await?;
    if workspace.tenant_id != tenant_id {
        return Err(
            YorishiroError::not_found(format!("workspace '{workspace_id}' was not found")).into(),
        );
    }
    Ok(workspace)
}

pub async fn list_workspaces(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
) -> Result<Json<Vec<identity_workspaces::Model>>, ApiError> {
    let workspaces = identity_workspaces::Entity::find()
        .filter(identity_workspaces::Column::TenantId.eq(auth.tenant_id))
        .all(&ctx.db)
        .await
        .map_err(|err| ApiError::from(YorishiroError::Internal(err.into())))?;
    Ok(Json(workspaces))
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    /// Cap on the number of entities this workspace may hold.
    /// Omit for unlimited.
    pub max_entities: Option<i32>,
    /// Schema to associate with this workspace.
    /// Omit to leave it unset.
    #[serde(default)]
    pub schema_id: Option<Uuid>,
}

pub async fn create_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let provider = embedding_provider(&ctx)?;
    let embedding_model = crate::services::embedding::model_name_from_env();
    let dimensions = provider.dimensions() as i32;

    let workspace = tenancy::create_workspace(
        &ctx.db,
        auth.tenant_id,
        &body.name,
        body.max_entities,
        body.schema_id,
        Some((&embedding_model, dimensions)),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(workspace)))
}

#[derive(Debug, Serialize)]
pub struct WorkspaceDetail {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub schema_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub entity_count: i64,
    pub relation_count: i64,
    /// Currently *active* schemas only (one per distinct schema name), not a raw row count, which would also include archived versions.
    pub schema_count: i64,
}

pub async fn get_workspace(
    State(ctx): State<AppContext>,
    authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    let workspace = get_workspace_in_tenant(&ctx, authorized.ctx.tenant_id, id).await?;

    let entity_count = content_entities::count(authorized.txn(), workspace.id).await?;
    let relation_count = content_relations::count(authorized.txn(), workspace.id).await?;
    let schema_count = content_schemas::count_active(authorized.txn(), workspace.id).await?;

    Ok(Json(WorkspaceDetail {
        id: workspace.id,
        tenant_id: workspace.tenant_id,
        name: workspace.name,
        max_entities: workspace.max_entities,
        schema_id: workspace.schema_id,
        created_at: workspace.created_at.into(),
        entity_count,
        relation_count,
        schema_count,
    }))
}

pub async fn delete_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    get_workspace_in_tenant(&ctx, auth.tenant_id, id).await?;
    tenancy::delete_workspace(&ctx.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/workspaces")
        .add("/", get(list_workspaces))
        .add("/", post(create_workspace))
        .add("/{id}", get(get_workspace))
        .add("/{id}", delete(delete_workspace))
}
