//! A workspace's own worker-class assignment.
//!
//! A workspace that wants its embedding-sync jobs to run on tenant-private or official-node compute instead of the shared pool assigns one here.
//! A workspace with none configured stays `WorkerClass::Shared`, so an existing deployment is unaffected until an operator sets one.

use crate::controllers::ApiError;
use crate::controllers::middleware::auth::require_scope;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::edition::ee::controllers::middleware::auth as authz;
use crate::edition::ee::dtos::worker_class::SetWorkerClassRequest;
use crate::edition::ee::models::workspace_worker_classes::{self, WorkerClassAssignment};

/// `PUT /api/workspace/worker-class`
/// Requires the destination tag in `settings.worker_targets` and a separately running worker serving that tag.
/// The deployment declaration does not guarantee that the worker is currently online.
#[cfg(feature = "openapi")]
fn set_worker_class_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/worker-class",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::WorkerClassRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["worker_class_update"]), "responses": {"204": serde_json::json!({"description": "Worker class saved"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn set_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetWorkerClassRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_worker_classes::set_as(&ctx, &auth_ctx, body.worker_class).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/worker-class`
#[cfg(feature = "openapi")]
fn get_worker_class_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/worker-class",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["worker_class_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::WorkerClassResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn get_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<WorkerClassAssignment>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = workspace_worker_classes::describe_as(&ctx.db, &auth_ctx)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found("no worker class assignment configured for this workspace")
        })?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/worker-class`
#[cfg(feature = "openapi")]
fn delete_worker_class_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/worker-class",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["worker_class_delete"]), "responses": {"204": serde_json::json!({"description": "Worker class reset"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn delete_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_worker_classes::clear_as(&ctx.db, &auth_ctx).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![
        set_worker_class_doc(),
        get_worker_class_doc(),
        delete_worker_class_doc(),
    ]
}

pub fn routes() -> Routes {
    Routes::new().prefix("api/workspace").add(
        "/worker-class",
        axum::routing::put(set_worker_class)
            .get(get_worker_class)
            .delete(delete_worker_class),
    )
}
