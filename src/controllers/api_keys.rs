//! REST routes for issuing, listing, rotating and revoking API keys.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
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

pub(crate) async fn create(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    headers: HeaderMap,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<impl aide::axum::IntoApiResponse, ApiError> {
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

pub(crate) async fn revoke(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<StatusCode, ApiError> {
    IdentityApiKeys::revoke_for_tenant(&ctx.db, &auth, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/api-keys")
        .add(
            "/",
            aide::axum::routing::post_with(create, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["credential_create"]),
                ))
                .response::<201, Json<CreateApiKeyResponse>>()
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/",
            aide::axum::routing::get_with(list, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["credential_list"]),
                ))
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::delete_with(revoke, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["credential_revoke"]),
                ))
                .response_with::<204, (), _>(|op| op.description("API key revoked"))
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}/rotate",
            aide::axum::routing::post_with(rotate, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["credential_rotate"]),
                ))
                .response::<201, Json<CreateApiKeyResponse>>()
                .response::<401, Json<super::openapi::ApiErrorBody>>()
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<404, Json<super::openapi::ApiErrorBody>>()
                .response::<409, Json<super::openapi::ApiErrorBody>>()
            }),
        )
}

async fn rotate(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<impl aide::axum::IntoApiResponse, ApiError> {
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
