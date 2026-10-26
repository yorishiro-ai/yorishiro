//! A workspace's own embedding provider assignment.
//!
//! A workspace that wants its vectors produced by a different backend than the deployment default (its own node, a different OpenAI-compatible endpoint) configures one here, the same shape `workspace_llm_keys` already gives LLM inference.
//! A workspace with none configured keeps using the deployment default, so an existing deployment is unaffected until an operator sets one.

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
use crate::edition::ee::dtos::embedding::SetEmbeddingKeyRequest;
use crate::edition::ee::models::workspace_embedding_keys::{self, EmbeddingKeyDescription};

/// `PUT /api/workspace/embedding-key`
#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/workspace/embedding-key", request_body = crate::edition::ee::controllers::openapi::EmbeddingKeyRequest, responses((status = 204, description = "Embedding provider saved"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn set_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetEmbeddingKeyRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;

    workspace_embedding_keys::configure(&ctx, auth_ctx.workspace_id, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/embedding-key`
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/workspace/embedding-key", responses((status = 200, body = crate::edition::ee::controllers::openapi::EmbeddingKeyResponse), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "enterprise"))]
async fn get_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<EmbeddingKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = workspace_embedding_keys::describe(&ctx.db, auth_ctx.workspace_id)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found("no embedding provider configured for this workspace")
        })?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/embedding-key`
#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/workspace/embedding-key", responses((status = 204, description = "Embedding provider removed"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise"))]
async fn delete_embedding_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_embedding_keys::remove(&ctx, auth_ctx.workspace_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new().prefix("api/workspace").add(
        "/embedding-key",
        axum::routing::put(set_embedding_key)
            .get(get_embedding_key)
            .delete(delete_embedding_key),
    )
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_set_embedding_key),
        crate::controllers::route_inventory::path_doc(__path_get_embedding_key),
        crate::controllers::route_inventory::path_doc(__path_delete_embedding_key),
    ]
}
