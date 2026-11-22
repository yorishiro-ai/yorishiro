//! REST routes for issuing, listing, rotating and revoking API keys.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
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
        .map(|date| crate::data::time::render(ctx, &date).fixed_offset());
    grant.revoked_at = grant
        .revoked_at
        .map(|date| crate::data::time::render(ctx, &date).fixed_offset());
    grant
}

#[cfg(feature = "openapi")]
fn create_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/api-keys",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateApiKeyRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["credential_create"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateApiKeyResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
pub(crate) async fn create(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    headers: HeaderMap,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    body.validate_workspace_selection(
        headers.contains_key(crate::models::api_keys::WORKSPACE_HEADER),
    )?;
    let scope = body.scope.unwrap_or(ApiKeyScope::Read);
    crate::models::iam::access::require_key_grant(&ctx.db, &auth, scope, body.audit).await?;
    let created = if body.tenant_scoped {
        IdentityApiKeys::create_delegated_tenant_api_key(
            &ctx.db,
            &auth,
            scope,
            body.audit,
            &body.name,
            body.expires_at,
        )
        .await?
    } else {
        IdentityApiKeys::create_delegated_api_key(
            &ctx.db,
            &auth,
            scope,
            body.audit,
            &body.name,
            body.expires_at,
        )
        .await?
    };
    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id: created.id,
            prefix: created.key_prefix,
            full_key: created.plaintext,
            name: created.name,
            created_at: crate::data::time::render(&ctx, &created.created_at),
            grant: render_grant(&ctx, created.grant),
        }),
    ))
}

#[cfg(feature = "openapi")]
fn list_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/api-keys",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["credential_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<ListApiKeysResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
pub(crate) async fn list(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
) -> Result<Json<ListApiKeysResponse>, ApiError> {
    let keys = IdentityApiKeys::list_with_grants_as(&ctx.db, &auth).await?;
    let mut records = Vec::with_capacity(keys.len());
    for (key, grant) in keys {
        records.push(ApiKeyRecord {
            id: key.id,
            prefix: key.key_prefix,
            name: key.name,
            created_at: crate::data::time::render(&ctx, &key.created_at),
            last_used_at: key
                .last_used_at
                .map(|time| crate::data::time::render(&ctx, &time)),
            grant: render_grant(&ctx, grant),
        });
    }
    Ok(Json(ListApiKeysResponse { keys: records }))
}

#[cfg(feature = "openapi")]
fn revoke_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/api-keys/{id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["credential_revoke"]), "responses": {"204": serde_json::json!({"description": "API key revoked"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn revoke(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    IdentityApiKeys::revoke_for_tenant(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![create_doc(), list_doc(), revoke_doc(), rotate_doc()]
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .prefix("api/api-keys")
        .add("/", post(create))
        .add("/", get(list))
        .add("/{id}", delete(revoke))
        .add("/{id}/rotate", post(rotate))
}

#[cfg(feature = "openapi")]
fn rotate_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/api-keys/{id}/rotate",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["credential_rotate"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateApiKeyResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
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
            created_at: crate::data::time::render(&ctx, &created.created_at),
            grant: render_grant(&ctx, created.grant),
        }),
    ))
}
