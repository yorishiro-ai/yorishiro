use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::members::require_tenant_admin;
use crate::error::{ValidationDetail, ValidationErrorCode, YorishiroError};
use crate::models::identity::api_keys::IdentityApiKeys;
use crate::services::auth::ApiKeyScope;

const MAX_NAME_LENGTH: usize = 100;

#[derive(Deserialize)]
pub(crate) struct CreateApiKeyRequest {
    pub(crate) name: String,
}

#[derive(Serialize)]
pub(crate) struct ApiKeyRecord {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub(crate) struct CreateApiKeyResponse {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) full_key: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub(crate) struct ListApiKeysResponse {
    pub(crate) keys: Vec<ApiKeyRecord>,
}

fn validate_name(name: &str) -> Result<String, YorishiroError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LENGTH {
        return Err(YorishiroError::ValidationFailed {
            message: "name must be between 1 and 100 characters".into(),
            details: vec![ValidationDetail {
                field: "name".into(),
                problem: "name must not be empty and must be at most 100 characters".into(),
                code: ValidationErrorCode::EmptyRequired,
                expected: Some("1-100 characters".into()),
                actual: Some(name.chars().count().to_string()),
            }],
            hint: "provide a non-empty API key name".into(),
        });
    }
    Ok(name.to_owned())
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/api-keys", request_body = super::openapi::CreateApiKeyRequest, responses((status = 201, body = super::openapi::CreateApiKeyResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
pub(crate) async fn create(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    let name = validate_name(&body.name)?;
    let created = IdentityApiKeys::create_named_api_key(
        &ctx.db,
        auth.workspace_id,
        ApiKeyScope::Migration,
        auth.user_id,
        false,
        &name,
    )
    .await?;
    let model = IdentityApiKeys::find_by_id(created.id)
        .one(&ctx.db)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?
        .ok_or_else(|| YorishiroError::not_found("created API key was not found"))?;

    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id: created.id,
            prefix: model.key_prefix,
            full_key: created.plaintext,
            name: model.name,
            created_at: model.created_at.into(),
        }),
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/api-keys", responses((status = 200, body = super::openapi::ListApiKeysResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
pub(crate) async fn list(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
) -> Result<Json<ListApiKeysResponse>, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    let keys = IdentityApiKeys::list_for_tenant(&ctx.db, auth.tenant_id).await?;
    Ok(Json(ListApiKeysResponse {
        keys: keys
            .into_iter()
            .map(|key| ApiKeyRecord {
                id: key.id,
                prefix: key.key_prefix,
                name: key.name,
                created_at: key.created_at.into(),
            })
            .collect(),
    }))
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/api-keys/{id}", params(("id" = Uuid, Path)), responses((status = 204, description = "API key revoked"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
pub(crate) async fn revoke(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    IdentityApiKeys::revoke_for_tenant(&ctx.db, auth.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_create),
        super::route_inventory::path_doc(__path_list),
        super::route_inventory::path_doc(__path_revoke),
    ]
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .prefix("api/api-keys")
        .add("/", post(create))
        .add("/", get(list))
        .add("/{id}", delete(revoke))
}
