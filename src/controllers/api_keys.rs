use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::members::require_tenant_admin;
use crate::dtos::api_keys::{
    ApiKeyRecord, CreateApiKeyRequest, CreateApiKeyResponse, ListApiKeysResponse,
};
use crate::models::api_keys::ApiKeyScope;
use crate::models::api_keys::IdentityApiKeys;

pub(crate) fn render_grant(
    ctx: &AppContext,
    mut grant: crate::models::api_keys::KeyGrant,
) -> crate::models::api_keys::KeyGrant {
    grant.expires_at = grant
        .expires_at
        .map(|date| crate::time::render(ctx, date).fixed_offset());
    grant.revoked_at = grant
        .revoked_at
        .map(|date| crate::time::render(ctx, date).fixed_offset());
    grant
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/api-keys", request_body = crate::dtos::api_keys::CreateApiKeyRequest, responses((status = 201, body = crate::dtos::api_keys::CreateApiKeyResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
pub(crate) async fn create(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let scope = body.scope.unwrap_or(ApiKeyScope::Read);
    crate::models::access::require_key_grant(&ctx.db, &auth, scope, body.audit).await?;
    let created = IdentityApiKeys::create_delegated_api_key(
        &ctx.db,
        &auth,
        scope,
        body.audit,
        &body.name,
        body.expires_at,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id: created.id,
            prefix: created.key_prefix,
            full_key: created.plaintext,
            name: created.name,
            created_at: crate::time::render(&ctx, created.created_at),
            grant: render_grant(&ctx, created.grant),
        }),
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/api-keys", responses((status = 200, body = crate::dtos::api_keys::ListApiKeysResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
pub(crate) async fn list(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
) -> Result<Json<ListApiKeysResponse>, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    let keys = IdentityApiKeys::list_for_tenant(&ctx.db, auth.tenant_id).await?;
    let mut records = Vec::with_capacity(keys.len());
    for key in keys {
        let grant = key.grant_metadata(&ctx.db).await?;
        records.push(ApiKeyRecord {
            id: key.id,
            prefix: key.key_prefix,
            name: key.name,
            created_at: crate::time::render(&ctx, key.created_at),
            last_used_at: key.last_used_at.map(|time| crate::time::render(&ctx, time)),
            grant: render_grant(&ctx, grant),
        });
    }
    Ok(Json(ListApiKeysResponse { keys: records }))
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/api-keys/{id}", params(("id" = Uuid, Path)), responses((status = 204, description = "API key revoked"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
pub(crate) async fn revoke(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    IdentityApiKeys::revoke_for_tenant(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_create),
        super::route_inventory::path_doc(__path_list),
        super::route_inventory::path_doc(__path_revoke),
        super::route_inventory::path_doc(__path_rotate),
    ]
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .prefix("api/api-keys")
        .add("/", post(create))
        .add("/", get(list))
        .add("/{id}", delete(revoke))
        .add("/{id}/rotate", post(rotate))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/api-keys/{id}/rotate", params(("id" = Uuid, Path)), responses((status = 201, body = crate::dtos::api_keys::CreateApiKeyResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
async fn rotate(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let created = IdentityApiKeys::rotate(&ctx.db, &auth, id).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id: created.id,
            prefix: created.key_prefix,
            full_key: created.plaintext,
            name: created.name,
            created_at: crate::time::render(&ctx, created.created_at),
            grant: render_grant(&ctx, created.grant),
        }),
    ))
}
