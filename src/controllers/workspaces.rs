//! REST routes for workspaces and their members.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
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
) -> Result<axum::response::Response, ApiError> {
    let workspace = tenancy::create_workspace_in_context(
        &ctx,
        &auth,
        &body.name,
        body.max_entities,
        body.schema_id,
    )
    .await?;
    Ok(axum::response::IntoResponse::into_response((
        StatusCode::CREATED,
        Json(workspace),
    )))
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
) -> Result<StatusCode, ApiError> {
    tenancy::delete_workspace_as(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn grant_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdUserIdPath { id, user_id }): Path<
        crate::dtos::common::IdUserIdPath<Uuid, Uuid>,
    >,
    Json(body): Json<crate::dtos::workspaces::GrantWorkspaceMemberRequest>,
) -> Result<StatusCode, ApiError> {
    crate::models::workspace_memberships::Model::grant_as(&ctx.db, &actor, id, user_id, body.role)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revoke_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdUserIdPath { id, user_id }): Path<
        crate::dtos::common::IdUserIdPath<Uuid, Uuid>,
    >,
) -> Result<StatusCode, ApiError> {
    crate::models::workspace_memberships::Model::revoke_as(&ctx.db, &actor, id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
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

#[expect(
    clippy::too_many_lines,
    reason = "Keep route registration and aide metadata together"
)]
pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/workspaces")
        .add(
            "/",
            aide::axum::routing::get_with(list_workspaces, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_list", "workspace_get"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_workspace, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_create"]),
                    ))
                    .response::<201, Json<WorkspaceRecord>>()
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
                    .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::get_with(get_workspace, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_get"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::put_with(update_workspace, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_update"]),
                    ))
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::delete_with(delete_workspace, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_delete"]),
                    ))
                    .response_with::<204, (), _>(|op| op.description("Workspace deleted"))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
                    .response::<409, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/members/{user_id}",
            aide::axum::routing::put_with(grant_member, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_update"]),
                    ))
                    .response_with::<204, (), _>(|op| {
                        op.description("Direct workspace grant saved")
                    })
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/members/{user_id}",
            aide::axum::routing::delete_with(revoke_member, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_update"]),
                    ))
                    .response_with::<204, (), _>(|op| {
                        op.description("Direct workspace grant and bound credentials revoked")
                    })
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/members",
            aide::axum::routing::get_with(list_members, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_get"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
            }),
        )
}
