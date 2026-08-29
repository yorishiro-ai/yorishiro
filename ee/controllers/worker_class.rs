//! A workspace's own worker-class assignment.
//!
//! A workspace that wants its embedding-sync jobs to run on tenant-private or official-node compute instead of the shared pool assigns one here.
//! A workspace with none configured stays `WorkerClass::Shared`, so an existing deployment is unaffected until an operator sets one.

use crate::controllers::ApiError;
use crate::error::YorishiroError;
use crate::services::auth::{ApiKeyScope, require_scope};
use crate::workers::embedding_sync::WorkerClass;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Deserialize;

use crate::ee::models::worker_classes::{self, WorkerClassAssignment};
use crate::ee::services::authz;

/// Base's own extractors enforce a minimum scope by type; without them here, the check is written out explicitly, matching `inference.rs`'s/`embedding.rs`'s own `require_scope`.
#[derive(Debug, Deserialize)]
pub struct SetWorkerClassRequest {
    pub worker_class: WorkerClass,
}

/// `PUT /api/workspace/worker-class`
async fn set_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetWorkerClassRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    worker_classes::set(&ctx.db, auth_ctx.workspace_id, body.worker_class).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/worker-class`
async fn get_worker_class(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<WorkerClassAssignment>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = worker_classes::describe(&ctx.db, auth_ctx.workspace_id)
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
    worker_classes::clear(&ctx.db, auth_ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new().prefix("api/workspace").add(
        "/worker-class",
        axum::routing::put(set_worker_class)
            .get(get_worker_class)
            .delete(delete_worker_class),
    )
}
