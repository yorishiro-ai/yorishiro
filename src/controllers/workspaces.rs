use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::extractors::{ReadScope, Verified};
use crate::dtos::workspaces::{CreateWorkspaceRequest, WorkspaceDetail};
use crate::models::workspace_workspaces;
use crate::models::workspace_workspaces::WorkspaceRecord;
use crate::models::{entity_entities, entity_relations, schema_schemas, tenancy};

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/workspaces", responses((status = 200, body = [crate::models::workspace_workspaces::WorkspaceRecord]), (status = 401, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), tag = "community"))]
pub(crate) async fn list_workspaces(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
) -> Result<Json<Vec<WorkspaceRecord>>, ApiError> {
    let records = crate::models::workspace_workspaces::list_for_actor(&ctx.db, &verified.ctx)
        .await?
        .into_iter()
        .map(|record| WorkspaceRecord {
            created_at: crate::time::in_timezone(record.created_at, crate::time::configured(&ctx))
                .fixed_offset(),
            ..record
        })
        .collect();
    Ok(Json(records))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/workspaces", request_body = crate::dtos::workspaces::CreateWorkspaceRequest, responses((status = 201, body = crate::models::workspace_workspaces::WorkspaceRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace = tenancy::create_workspace_in_context(
        &ctx,
        &auth,
        &body.name,
        body.max_entities,
        body.schema_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(workspace)))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/workspaces/{id}", params(("id" = Uuid, Path)), responses((status = 200, body = crate::dtos::workspaces::WorkspaceDetail), (status = 401, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn get_workspace(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    let workspace = workspace_workspaces::get_for_actor(&ctx.db, &verified.ctx, id).await?;
    let txn = crate::db::begin_workspace(&ctx, workspace.tenant_id, workspace.id).await?;

    let entity_count = entity_entities::count(&txn, workspace.id).await?;
    let relation_count = entity_relations::count(&txn, workspace.id).await?;
    let schema_count = schema_schemas::count_active(&txn, workspace.id).await?;

    let timezone = crate::time::configured(&ctx);
    let quotas = crate::quotas(&ctx, workspace.tenant_id).await?;
    Ok(Json(WorkspaceDetail {
        id: workspace.id,
        tenant_id: workspace.tenant_id,
        name: workspace.name,
        max_entities: workspace.max_entities,
        effective_max_entities: crate::data::quotas::effective_limit(
            workspace.max_entities,
            quotas.entities,
        ),
        schema_id: workspace.schema_id,
        created_at: crate::time::in_timezone(workspace.created_at, timezone),
        entity_count,
        relation_count,
        schema_count,
    }))
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/workspaces/{id}", params(("id" = Uuid, Path)), responses((status = 204, description = "Workspace deleted"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
pub(crate) async fn delete_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    tenancy::delete_workspace_as(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_list_workspaces),
        super::route_inventory::path_doc(__path_create_workspace),
        super::route_inventory::path_doc(__path_get_workspace),
        super::route_inventory::path_doc(__path_delete_workspace),
        super::route_inventory::path_doc(__path_grant_member),
        super::route_inventory::path_doc(__path_revoke_member),
    ]
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/workspaces/{id}/members/{user_id}", params(("id" = Uuid, Path), ("user_id" = Uuid, Path)), request_body = crate::dtos::workspaces::GrantWorkspaceMemberRequest, responses((status = 204, description = "Direct workspace grant saved"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["workspace_admin"]))), tag = "community"))]
async fn grant_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<crate::dtos::workspaces::GrantWorkspaceMemberRequest>,
) -> Result<StatusCode, ApiError> {
    crate::models::workspace_memberships::Model::grant_as(&ctx.db, &actor, id, user_id, body.role)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/workspaces/{id}/members/{user_id}", params(("id" = Uuid, Path), ("user_id" = Uuid, Path)), responses((status = 204, description = "Direct workspace grant and bound credentials revoked"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["workspace_admin"]))), tag = "community"))]
async fn revoke_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::workspace_memberships::Model::revoke_as(&ctx.db, &actor, id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/workspaces")
        .add("/", get(list_workspaces))
        .add("/", post(create_workspace))
        .add("/{id}", get(get_workspace))
        .add("/{id}", delete(delete_workspace))
        .add("/{id}/members/{user_id}", axum::routing::put(grant_member))
        .add("/{id}/members/{user_id}", delete(revoke_member))
}
