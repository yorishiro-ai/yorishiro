//! Which columns the Entities table shows, chosen per workspace and entity type. Ported from
//! master's `ee/crates/yorishiro-hosted/src/http/controllers/entity_columns.rs`, at the same
//! `/api/workspace/entity-columns` paths: base's own workspace routes are mounted at
//! `api/workspaces` (plural), so this does not collide.
//!
//! Reading needs `read`, writing needs `write`: this is a display preference, not schema state,
//! so a key that may create entities may also decide how they are listed.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Deserialize;
use yorishiro_core::controllers::ApiError;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::services::auth::{ApiKeyScope, AuthContext};

use crate::models::entity_columns::{self, ColumnPreference};
use crate::services::authz;

/// Base's own extractors enforce a minimum scope by type. Without them, the check is written
/// out: the ordering on `ApiKeyScope` is the same one they use.
fn require_scope(ctx: &AuthContext, needed: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope < needed {
        return Err(YorishiroError::ScopeInsufficient {
            message: format!("this endpoint needs the {needed:?} scope or higher"),
            hint: "issue a key with a higher scope".into(),
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SetColumnsRequest {
    /// Field names from the schema, in the order they should be displayed. An empty list is a
    /// choice ("show no fields"), distinct from never having chosen, which is what `DELETE`
    /// restores.
    pub columns: Vec<String>,
}

/// `GET /api/workspace/entity-columns`
async fn list_columns(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<Vec<ColumnPreference>>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let db = ctx
        .shared_store
        .get::<yorishiro_core::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?;
    let stored = entity_columns::list(&schema_txn, auth_ctx.workspace_id).await?;
    Ok(Json(stored))
}

/// `PUT /api/workspace/entity-columns/{entity_type}`
async fn set_columns(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
    Json(body): Json<SetColumnsRequest>,
) -> Result<Json<ColumnPreference>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Write)?;
    let db = ctx
        .shared_store
        .get::<yorishiro_core::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?;
    let stored = entity_columns::set(
        &schema_txn,
        auth_ctx.workspace_id,
        &entity_type,
        &body.columns,
    )
    .await?;
    schema_txn.commit().await.internal()?;
    Ok(Json(stored))
}

/// `DELETE /api/workspace/entity-columns/{entity_type}`
async fn reset_columns(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Write)?;
    let db = ctx
        .shared_store
        .get::<yorishiro_core::db::DbHandle>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("DbHandle missing")))?;
    let schema_txn = db
        .tenant
        .begin_for_workspace(auth_ctx.tenant_id, auth_ctx.workspace_id)
        .await
        .internal()?;
    entity_columns::clear(&schema_txn, auth_ctx.workspace_id, &entity_type).await?;
    schema_txn.commit().await.internal()?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/workspace")
        .add("/entity-columns", axum::routing::get(list_columns))
        .add(
            "/entity-columns/{entity_type}",
            axum::routing::put(set_columns).delete(reset_columns),
        )
}
