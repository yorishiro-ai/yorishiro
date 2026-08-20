//! Which columns the Entities table shows, chosen per workspace and entity type.
//!
//! Reading needs `read`, writing needs `write`: this is a display preference, not schema state, so a key that may create entities may also decide how they are listed.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use utoipa::ToSchema;
use yorishiro_core::ResultExt;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::services::auth::{ApiKeyScope, AuthContext};

use crate::error::HostedApiError;
use crate::models::entity_columns::{self, ColumnPreference};
use crate::services::authz;
use crate::state::HostedState;

/// The community edition's `Authorized<Scope>` extractor is not reachable from here, so the scope check is written out.
/// Same comparison it makes: `ApiKeyScope` is ordered, and a key carrying a higher scope satisfies a lower requirement.
fn require_scope(ctx: &AuthContext, needed: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope < needed {
        return Err(YorishiroError::ScopeInsufficient {
            message: format!("this endpoint needs the {needed:?} scope or higher"),
            hint: "issue a key with a higher scope".into(),
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetColumnsRequest {
    /// Field names from the schema, in the order they should be displayed.
    /// An empty list is a choice ("show no fields"), distinct from never having chosen, which is what `DELETE` restores.
    pub columns: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/api/workspace/entity-columns",
    responses(
        (status = 200, description = "Every stored preference in the workspace", body = Vec<ColumnPreference>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::HostedApiErrorBody),
    ),
    tag = "entity-columns",
)]
pub async fn list_columns(
    State(state): State<HostedState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ColumnPreference>>, HostedApiError> {
    let ctx = authz::authenticate_workspace(&state, &headers).await?;
    require_scope(&ctx, ApiKeyScope::Read)?;
    let mut conn = state.identity_pool.acquire().await.internal()?;
    let stored = entity_columns::list(&mut conn, ctx.workspace_id).await?;
    Ok(Json(stored))
}

#[utoipa::path(
    put,
    path = "/api/workspace/entity-columns/{entity_type}",
    params(("entity_type" = String, Path, description = "The entity type these columns belong to")),
    request_body = SetColumnsRequest,
    responses(
        (status = 200, description = "Stored", body = ColumnPreference),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::HostedApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::HostedApiErrorBody),
        (status = 422, description = "Too many columns, or one listed twice", body = crate::error::HostedApiErrorBody),
    ),
    tag = "entity-columns",
)]
pub async fn set_columns(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
    Json(request): Json<SetColumnsRequest>,
) -> Result<Json<ColumnPreference>, HostedApiError> {
    let ctx = authz::authenticate_workspace(&state, &headers).await?;
    require_scope(&ctx, ApiKeyScope::Write)?;
    let mut conn = state.identity_pool.acquire().await.internal()?;
    let stored =
        entity_columns::set(&mut conn, ctx.workspace_id, &entity_type, &request.columns).await?;
    Ok(Json(stored))
}

#[utoipa::path(
    delete,
    path = "/api/workspace/entity-columns/{entity_type}",
    params(("entity_type" = String, Path, description = "The entity type to reset")),
    responses(
        (status = 204, description = "Reset to the schema-derived default"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::HostedApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::HostedApiErrorBody),
    ),
    tag = "entity-columns",
)]
pub async fn reset_columns(
    State(state): State<HostedState>,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
) -> Result<StatusCode, HostedApiError> {
    let ctx = authz::authenticate_workspace(&state, &headers).await?;
    require_scope(&ctx, ApiKeyScope::Write)?;
    let mut conn = state.identity_pool.acquire().await.internal()?;
    entity_columns::clear(&mut conn, ctx.workspace_id, &entity_type).await?;
    Ok(StatusCode::NO_CONTENT)
}
