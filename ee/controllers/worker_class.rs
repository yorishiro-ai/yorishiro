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

use crate::ee::controllers::middleware::auth as authz;
use crate::ee::dtos::worker_class::SetWorkerClassRequest;
use crate::ee::models::workspace_worker_classes::{self, WorkerClassAssignment};

/// `PUT /api/workspace/worker-class`
#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/workspace/worker-class", request_body = crate::ee::controllers::openapi::WorkerClassRequest, responses((status = 204, description = "Worker class saved"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn set_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetWorkerClassRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_worker_classes::set(&ctx.db, auth_ctx.workspace_id, body.worker_class).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/worker-class`
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/workspace/worker-class", responses((status = 200, body = crate::ee::controllers::openapi::WorkerClassResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "enterprise"))]
async fn get_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<WorkerClassAssignment>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = workspace_worker_classes::describe(&ctx.db, auth_ctx.workspace_id)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found("no worker class assignment configured for this workspace")
        })?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/worker-class`
#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/workspace/worker-class", responses((status = 204, description = "Worker class reset"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn delete_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_worker_classes::clear(&ctx.db, auth_ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_set_worker_class),
        crate::controllers::route_inventory::path_doc(__path_get_worker_class),
        crate::controllers::route_inventory::path_doc(__path_delete_worker_class),
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
