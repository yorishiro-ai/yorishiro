//! A workspace's own embedding provider assignment.
//!
//! A workspace that wants its vectors produced by a different backend than the deployment default (its own node, a different OpenAI-compatible endpoint) configures one here, the same shape `identity_workspace_llm_keys` already gives LLM inference.
//! A workspace with none configured keeps using the deployment default, so an existing deployment is unaffected until an operator sets one.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::EntityTrait;
use serde::Deserialize;
use yorishiro_core::controllers::ApiError;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::services::auth::ApiKeyScope;

use crate::models::embedding_keys::{self, EmbeddingKeyDescription};
use crate::services::authz;

/// Base's own extractors enforce a minimum scope by type; without them here, the check is written out explicitly, matching `inference.rs`'s own `require_scope`.
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
pub struct SetEmbeddingKeyRequest {
    /// An OpenAI-compatible embeddings endpoint, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub model: String,
    /// Stored as given and never returned. `GET` reports only that one is configured.
    pub api_key: String,
    pub dimensions: i32,
    /// Some OpenAI-compatible implementations don't recognize the `dimensions` request parameter.
    #[serde(default)]
    pub send_dimensions_param: bool,
}

/// `PUT /hosted/workspace/embedding-key`
async fn set_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetEmbeddingKeyRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;

    let workspace = yorishiro_core::models::_entities::identity_workspaces::Entity::find_by_id(
        auth_ctx.workspace_id,
    )
    .one(&ctx.db)
    .await
    .map_err(|err| ApiError(YorishiroError::Internal(err.into())))?
    .ok_or_else(|| YorishiroError::not_found("workspace not found"))?;

    embedding_keys::set(
        &ctx.db,
        auth_ctx.workspace_id,
        &body.base_url,
        &body.model,
        &body.api_key,
        body.dimensions,
        body.send_dimensions_param,
        workspace.embedding_dimensions,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /hosted/workspace/embedding-key`
async fn get_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<EmbeddingKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = embedding_keys::describe(&ctx.db, auth_ctx.workspace_id)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found("no embedding provider configured for this workspace")
        })?;
    Ok(Json(described))
}

/// `DELETE /hosted/workspace/embedding-key`
async fn delete_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    embedding_keys::clear(&ctx.db, auth_ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new().prefix("hosted").add(
        "/workspace/embedding-key",
        axum::routing::put(set_embedding_key)
            .get(get_embedding_key)
            .delete(delete_embedding_key),
    )
}
