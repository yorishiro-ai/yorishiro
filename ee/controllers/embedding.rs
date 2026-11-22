//! A workspace's own embedding provider assignment.
//!
//! A workspace that wants its vectors produced by a different backend than the deployment default (its own node, a different OpenAI-compatible endpoint) configures one here, the same shape `workspace_llm_keys` already gives LLM inference.
//! A workspace with none configured keeps using the deployment default, so an existing deployment is unaffected until an operator sets one.

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
use crate::edition::ee::dtos::embedding::SetEmbeddingKeyRequest;
use crate::edition::ee::models::workspace_embedding_keys::{self, EmbeddingKeyDescription};

/// `PUT /api/workspace/embedding-key`
async fn set_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetEmbeddingKeyRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;

    workspace_embedding_keys::configure(&ctx, &auth_ctx, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/embedding-key`
async fn get_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<EmbeddingKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = workspace_embedding_keys::describe_as(&ctx.db, &auth_ctx)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found("no embedding provider configured for this workspace")
        })?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/embedding-key`
async fn delete_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_embedding_keys::remove(&ctx, &auth_ctx).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/workspace")
        .add(
            "/embedding-key",
            aide::axum::routing::put_with(set_embedding_key, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["embedding_provider_update"]),
                ))
                .response_with::<204, (), _>(|op| op.description("Embedding provider saved"))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/embedding-key",
            aide::axum::routing::get_with(get_embedding_key, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["embedding_provider_get"]),
                ))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/embedding-key",
            aide::axum::routing::delete_with(delete_embedding_key, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["schema"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["embedding_provider_delete"]),
                ))
                .response_with::<204, (), _>(|op| op.description("Embedding provider removed"))
                .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                .response::<403, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
}
