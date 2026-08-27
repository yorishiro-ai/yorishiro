//! A workspace's own worker-class assignment.
//!
//! A workspace that wants its embedding-sync jobs to run on tenant-private or official-node compute instead of the shared pool assigns one here.
//! A workspace with none configured stays `WorkerClass::Shared`, so an existing deployment is unaffected until an operator sets one.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Deserialize;
use yorishiro_core::controllers::ApiError;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::services::auth::ApiKeyScope;
use yorishiro_core::workers::embedding_sync::WorkerClass;

use crate::models::worker_classes::{self, WorkerClassAssignment};
use crate::services::authz;

/// Base's own extractors enforce a minimum scope by type; without them here, the check is written out explicitly, matching `inference.rs`'s/`embedding.rs`'s own `require_scope`.
fn require_scope(
    ctx: &yorishiro_core::services::auth::AuthContext,
    needed: ApiKeyScope,
) -> Result<(), YorishiroError> {
    if ctx.scope < needed {
        return Err(YorishiroError::ScopeInsufficient {
            message: format!("this endpoint needs the {needed:?} scope or higher"),
            hint: "issue a key with a higher scope".into(),
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SetWorkerClassRequest {
    pub worker_class: WorkerClass,
}

/// `PUT /hosted/workspace/worker-class`
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

/// `GET /hosted/workspace/worker-class`
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

/// `DELETE /hosted/workspace/worker-class`
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
    Routes::new().prefix("hosted").add(
        "/workspace/worker-class",
        axum::routing::put(set_worker_class)
            .get(get_worker_class)
            .delete(delete_worker_class),
    )
}
