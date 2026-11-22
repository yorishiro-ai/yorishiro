//! A workspace's own worker-class assignment.
//!
//! A workspace that wants its embedding-sync jobs to run on tenant-private or official-node compute instead of the shared pool assigns one here.
//! A workspace with none configured stays `WorkerClass::Shared`, so an existing deployment is unaffected until an operator sets one.

use crate::controllers::ApiError;
use crate::controllers::middleware::auth::require_scope;
use crate::controllers::openapi::Routes;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;

use crate::edition::ee::controllers::middleware::auth as authz;
use crate::edition::ee::dtos::worker_class::SetWorkerClassRequest;
use crate::edition::ee::models::workspace_worker_classes::{self, WorkerClassAssignment};

/// `PUT /api/workspace/worker-class`
/// Requires the destination tag in `settings.worker_targets` and a separately running worker serving that tag.
/// The deployment declaration does not guarantee that the worker is currently online.
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
async fn delete_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_worker_classes::clear_as(&ctx.db, &auth_ctx).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/workspace")
        .add(
            "/worker-class",
            aide::axum::routing::put_with(set_worker_class, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["worker_class_update"]),
                ))
                .response_with::<204, (), _>(|op| op.description("Worker class saved"))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/worker-class",
            aide::axum::routing::get_with(get_worker_class, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["worker_class_get"]),
                ))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/worker-class",
            aide::axum::routing::delete_with(delete_worker_class, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["worker_class_delete"]),
                ))
                .response_with::<204, (), _>(|op| op.description("Worker class reset"))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
}
