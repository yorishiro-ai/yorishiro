//! A workspace's own embedding provider assignment.
//!
//! A workspace that wants its vectors produced by a different backend than the deployment default (its own node, a different OpenAI-compatible endpoint) configures one here, the same shape `workspace_llm_keys` already gives LLM inference.
//! A workspace with none configured keeps using the deployment default, so an existing deployment is unaffected until an operator sets one.

use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use crate::edition::ee::controllers::middleware::auth::WorkspaceAuthenticated;
use crate::error::YorishiroError;
use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::edition::ee::dtos::embedding::SetEmbeddingKeyRequest;
use crate::edition::ee::models::workspace_embedding_keys::{self, EmbeddingKeyDescription};

/// `PUT /api/workspace/embedding-provider`
async fn set_embedding_key(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
    Json(body): Json<SetEmbeddingKeyRequest>,
) -> Result<axum::response::NoContent, ApiError> {
    workspace_embedding_keys::configure(&ctx, &auth_ctx, body).await?;
    Ok(axum::response::NoContent)
}

/// `GET /api/workspace/embedding-provider`
async fn get_embedding_key(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
) -> Result<Json<EmbeddingKeyDescription>, ApiError> {
    let described = workspace_embedding_keys::describe_as(&ctx.db, &auth_ctx)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found("no embedding provider configured for this workspace")
        })?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/embedding-provider`
async fn delete_embedding_key(
    State(ctx): State<AppContext>,
    WorkspaceAuthenticated(auth_ctx): WorkspaceAuthenticated,
) -> Result<axum::response::NoContent, ApiError> {
    workspace_embedding_keys::remove(&ctx, &auth_ctx).await?;
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/workspace")
        .add(
            "/embedding-provider",
            aide::axum::routing::put_with(set_embedding_key, |op| {
                op.id("UpdateWorkspaceEmbeddingProvider")
            }),
        )
        .add(
            "/embedding-provider",
            aide::axum::routing::get_with(get_embedding_key, |op| {
                op.id("GetWorkspaceEmbeddingProvider")
            }),
        )
        .add(
            "/embedding-provider",
            aide::axum::routing::delete_with(delete_embedding_key, |op| {
                op.id("DeleteWorkspaceEmbeddingProvider")
            }),
        )
}
