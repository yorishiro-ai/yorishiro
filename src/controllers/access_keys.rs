//! REST routes for issuing, listing, updating and deleting access keys.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::dtos::access_keys::{
    AccessKeyIdPath, AccessKeyRecord, CreateAccessKeyRequest, CreateAccessKeyResponse,
    ListAccessKeysResponse, UpdateAccessKeyRequest,
};
use crate::models::access_keys::IdentityAccessKeys;

pub(crate) fn render_grant(
    ctx: &AppContext,
    mut grant: crate::models::access_keys::KeyGrant,
) -> crate::models::access_keys::KeyGrant {
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
    Json(body): Json<CreateAccessKeyRequest>,
) -> Result<impl aide::axum::IntoApiResponse, ApiError> {
    body.validate_workspace_selection(
        headers.contains_key(crate::models::access_keys::WORKSPACE_HEADER),
    )?;
    let created = if body.tenant_scoped {
        IdentityAccessKeys::create_delegated_tenant_access_key(
            &ctx.db,
            &auth,
            &body.name,
            body.expires_at,
        )
        .await?
    } else {
        IdentityAccessKeys::create_delegated_access_key(&ctx.db, &auth, &body.name, body.expires_at)
            .await?
    };
    Ok(Created(CreateAccessKeyResponse {
        id: created.id,
        prefix: created.key_prefix,
        full_key: created.plaintext,
        name: created.name,
        created_at: crate::data::time::render(&ctx, &created.created_at),
        grant: render_grant(&ctx, created.grant),
    }))
}

pub(crate) fn record(
    ctx: &AppContext,
    key: crate::models::access_keys::Model,
    grant: crate::models::access_keys::KeyGrant,
) -> AccessKeyRecord {
    AccessKeyRecord {
        id: key.id,
        prefix: key.key_prefix,
        name: key.name,
        created_at: crate::data::time::render(ctx, &key.created_at),
        last_used_at: key
            .last_used_at
            .map(|time| crate::data::time::render(ctx, &time)),
        grant: render_grant(ctx, grant),
    }
}

pub(crate) async fn list(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
) -> Result<Json<ListAccessKeysResponse>, ApiError> {
    let keys = IdentityAccessKeys::list_with_grants_as(&ctx.db, &auth).await?;
    let records = keys
        .into_iter()
        .map(|(key, grant)| record(&ctx, key, grant))
        .collect();
    Ok(Json(ListAccessKeysResponse { keys: records }))
}

pub(crate) async fn update(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(AccessKeyIdPath { access_key_id }): Path<AccessKeyIdPath>,
    Json(body): Json<UpdateAccessKeyRequest>,
) -> Result<Json<AccessKeyRecord>, ApiError> {
    let (key, grant) =
        IdentityAccessKeys::update_as(&ctx.db, &auth, access_key_id, body.into_change()).await?;
    Ok(Json(record(&ctx, key, grant)))
}

pub(crate) async fn delete(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(AccessKeyIdPath { access_key_id }): Path<AccessKeyIdPath>,
) -> Result<axum::response::NoContent, ApiError> {
    IdentityAccessKeys::delete_as(&ctx.db, &auth, access_key_id).await?;
    Ok(axum::response::NoContent)
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/access-keys")
        .add(
            "/",
            aide::axum::routing::post_with(create, |op| op.id("CreateAccessKey")),
        )
        .add(
            "/",
            aide::axum::routing::get_with(list, |op| op.id("ListAccessKeys")),
        )
        .add(
            "/{access_key_id}",
            aide::axum::routing::patch_with(update, |op| op.id("UpdateAccessKey")),
        )
        .add(
            "/{access_key_id}",
            aide::axum::routing::delete_with(delete, |op| op.id("DeleteAccessKey")),
        )
}
