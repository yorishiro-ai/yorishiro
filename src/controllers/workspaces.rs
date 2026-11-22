//! REST routes for workspaces and their members.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
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

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn create_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<Created<WorkspaceRecord>, ApiError> {
    let workspace = tenancy::create_workspace_in_context(
        &ctx,
        &auth,
        &body.name,
        body.max_entities,
        body.schema_id,
    )
    .await?;
    Ok(Created(WorkspaceRecord::try_from(workspace)?))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_workspace(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    Ok(Json(
        workspace_workspaces::detail_for_actor(&ctx, &actor, id).await?,
    ))
}

async fn update_workspace(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<crate::dtos::workspaces::UpdateWorkspaceRequest>,
) -> Result<Json<WorkspaceRecord>, ApiError> {
    Ok(Json(
        workspace_workspaces::update_as(&ctx.db, &actor, id, body).await?,
    ))
}

pub(crate) async fn delete_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    tenancy::delete_workspace_as(&ctx.db, &auth, id).await?;
    Ok(axum::response::NoContent)
}

async fn grant_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdUserIdPath { id, user_id }): Path<
        crate::dtos::common::IdUserIdPath<Uuid, Uuid>,
    >,
    Json(body): Json<crate::dtos::workspaces::GrantWorkspaceMemberRequest>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::workspace_memberships::Model::grant_as(&ctx.db, &actor, id, user_id, body.role)
        .await?;
    Ok(axum::response::NoContent)
}

async fn revoke_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdUserIdPath { id, user_id }): Path<
        crate::dtos::common::IdUserIdPath<Uuid, Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::workspace_memberships::Model::revoke_as(&ctx.db, &actor, id, user_id).await?;
    Ok(axum::response::NoContent)
}

async fn list_members(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<Json<Vec<crate::dtos::workspaces::WorkspaceMemberRecord>>, ApiError> {
    Ok(Json(
        crate::models::workspace_memberships::Model::list_as(&ctx.db, &actor, id).await?,
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/workspaces")
        .add(
            "/",
            aide::axum::routing::get_with(list_workspaces, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_list", "workspace_get"]),
                ))
            }),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_workspace, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_create"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::get_with(get_workspace, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_get"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::put_with(update_workspace, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_update"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::delete_with(delete_workspace, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_delete"]),
                ))
            }),
        )
        .add(
            "/{id}/members/{user_id}",
            aide::axum::routing::put_with(grant_member, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_update"]),
                ))
            }),
        )
        .add(
            "/{id}/members/{user_id}",
            aide::axum::routing::delete_with(revoke_member, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_update"]),
                ))
                .response_with::<204, (), _>(|op| {
                    op.description("Direct workspace grant and bound credentials revoked")
                })
            }),
        )
        .add(
            "/{id}/members",
            aide::axum::routing::get_with(list_members, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["workspace_get"]),
                ))
            }),
        )
}
