//! REST routes for workspaces and their members.

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
use crate::dtos::workspaces::{CreateWorkspaceRequest, WorkspaceDetail};
use crate::models::tenancy;
use crate::models::workspace_workspaces;
use crate::models::workspace_workspaces::WorkspaceRecord;

#[cfg(feature = "openapi")]
fn list_workspaces_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["workspace_list", "workspace_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<WorkspaceRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
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

#[cfg(feature = "openapi")]
fn create_workspace_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateWorkspaceRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["workspace_create"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<WorkspaceRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn create_workspace(
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

#[cfg(feature = "openapi")]
fn get_workspace_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces/{id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["workspace_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<WorkspaceDetail>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_workspace(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    Ok(Json(
        workspace_workspaces::detail_for_actor(&ctx, &actor, id).await?,
    ))
}

#[cfg(feature = "openapi")]
fn update_workspace_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces/{id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::workspaces::UpdateWorkspaceRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["workspace_update"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<WorkspaceRecord>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn update_workspace(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(id): Path<Uuid>,
    Json(body): Json<crate::dtos::workspaces::UpdateWorkspaceRequest>,
) -> Result<Json<WorkspaceRecord>, ApiError> {
    Ok(Json(
        workspace_workspaces::update_as(&ctx.db, &actor, id, body).await?,
    ))
}

#[cfg(feature = "openapi")]
fn delete_workspace_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces/{id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["workspace_delete"]), "responses": {"204": serde_json::json!({"description": "Workspace deleted"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "Last workspace or referenced by schema forks", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn delete_workspace(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    tenancy::delete_workspace_as(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        list_workspaces_doc(),
        create_workspace_doc(),
        get_workspace_doc(),
        update_workspace_doc(),
        delete_workspace_doc(),
        grant_member_doc(),
        list_members_doc(),
        revoke_member_doc(),
    ]
}

#[cfg(feature = "openapi")]
fn grant_member_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces/{id}/members/{user_id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::workspaces::GrantWorkspaceMemberRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["workspace_update"]), "responses": {"204": serde_json::json!({"description": "Direct workspace grant saved"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn revoke_member_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces/{id}/members/{user_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["workspace_update"]), "responses": {"204": serde_json::json!({"description": "Direct workspace grant and bound credentials revoked"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}, {"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn revoke_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::workspace_memberships::Model::revoke_as(&ctx.db, &actor, id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn list_members_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspaces/{id}/members",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-actions" :  serde_json::json!(["workspace_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<crate::dtos::workspaces::WorkspaceMemberRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
async fn list_members(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<crate::dtos::workspaces::WorkspaceMemberRecord>>, ApiError> {
    Ok(Json(
        crate::models::workspace_memberships::Model::list_as(&ctx.db, &actor, id).await?,
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/workspaces")
        .add("/", get(list_workspaces))
        .add("/", post(create_workspace))
        .add("/{id}", get(get_workspace))
        .add("/{id}", axum::routing::put(update_workspace))
        .add("/{id}", delete(delete_workspace))
        .add("/{id}/members/{user_id}", axum::routing::put(grant_member))
        .add("/{id}/members/{user_id}", delete(revoke_member))
        .add("/{id}/members", get(list_members))
}
