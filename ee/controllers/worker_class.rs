//! A workspace's own worker-class assignment.
//!
//! A workspace that wants its embedding-sync jobs to run on tenant-private or official-node compute instead of the shared pool assigns one here.
//! A workspace with none configured stays `WorkerClass::Shared`, so an existing deployment is unaffected until an operator sets one.

use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use crate::edition::ee::controllers::middleware::auth::WorkspaceAuthenticated;
use crate::error::YorishiroError;
use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::edition::ee::dtos::worker_class::SetWorkerClassRequest;
use crate::edition::ee::models::workspace_worker_classes::{self, WorkerClassAssignment};

/// `PUT /api/workspace/worker-class`
/// Requires the destination tag in `settings.worker_targets` and a separately running worker serving that tag.
/// The deployment declaration does not guarantee that the worker is currently online.
async fn set_worker_class(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
    Json(body): Json<SetWorkerClassRequest>,
) -> Result<axum::response::NoContent, ApiError> {
    workspace_worker_classes::set_as(&ctx, &auth_ctx, body.worker_class).await?;
    Ok(axum::response::NoContent)
}

/// `GET /api/workspace/worker-class`
async fn get_worker_class(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
) -> Result<Json<WorkerClassAssignment>, ApiError> {
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
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
) -> Result<axum::response::NoContent, ApiError> {
    workspace_worker_classes::clear_as(&ctx.db, &auth_ctx).await?;
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/workspace")
        .add(
            "/worker-class",
            aide::axum::routing::put_with(set_worker_class, |op| {
                op.id("UpdateWorkspaceWorkerClass")
            }),
        )
        .add(
            "/worker-class",
            aide::axum::routing::get_with(get_worker_class, |op| op.id("GetWorkspaceWorkerClass")),
        )
        .add(
            "/worker-class",
            aide::axum::routing::delete_with(delete_worker_class, |op| {
                op.id("DeleteWorkspaceWorkerClass")
            }),
        )
}
