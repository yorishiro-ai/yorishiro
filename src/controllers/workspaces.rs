//! REST routes for workspaces.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::dtos::common::WorkspaceIdPath;
use crate::dtos::workspaces::{CreateWorkspaceRequest, WorkspaceDetail};
use crate::models::tenancy;
use crate::models::workspace_workspaces;
use crate::models::workspace_workspaces::WorkspaceRecord;

pub(crate) async fn list_workspaces(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
) -> Result<Json<Vec<WorkspaceRecord>>, ApiError> {
    let records = workspace_workspaces::list_for_actor(&ctx.db, &actor)
        .await?
        .into_iter()
        .map(|record| WorkspaceRecord {
            created_at: crate::data::time::render(&ctx, &record.created_at).fixed_offset(),
            ..record
        })
        .collect();
    Ok(Json(records))
}

async fn create_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<Created<WorkspaceRecord>, ApiError> {
    let workspace =
        tenancy::create_workspace_in_context(&ctx, &auth, &body.name, body.max_entities).await?;
    Ok(Created(WorkspaceRecord::try_from(workspace)?))
}

async fn get_workspace(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(WorkspaceIdPath { workspace_id }): Path<WorkspaceIdPath<Uuid>>,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    Ok(Json(
        workspace_workspaces::detail_for_actor(&ctx, &actor, workspace_id).await?,
    ))
}

async fn update_workspace(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(WorkspaceIdPath { workspace_id }): Path<WorkspaceIdPath<Uuid>>,
    Json(body): Json<crate::dtos::workspaces::UpdateWorkspaceRequest>,
) -> Result<Json<WorkspaceRecord>, ApiError> {
    Ok(Json(
        workspace_workspaces::update_as(&ctx.db, &actor, workspace_id, body).await?,
    ))
}

pub(crate) async fn delete_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(WorkspaceIdPath { workspace_id }): Path<WorkspaceIdPath<Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    tenancy::delete_workspace_as(&ctx.db, &auth, workspace_id).await?;
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/workspaces")
        .add(
            "/",
            aide::axum::routing::get_with(list_workspaces, |op| op.id("ListWorkspaces")),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_workspace, |op| op.id("CreateWorkspace")),
        )
        .add(
            "/{workspace_id}",
            aide::axum::routing::get_with(get_workspace, |op| op.id("GetWorkspace")),
        )
        .add(
            "/{workspace_id}",
            aide::axum::routing::put_with(update_workspace, |op| op.id("UpdateWorkspace")),
        )
        .add(
            "/{workspace_id}",
            aide::axum::routing::delete_with(delete_workspace, |op| op.id("DeleteWorkspace")),
        )
}
