//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::error::ResultExt;
use crate::services::auth::{ApiKeyScope, require_scope};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;
use uuid::Uuid;

use crate::ee::models::workspace_schema_forks as model;
use crate::ee::services::authz;

async fn list(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<Vec<model::ForkRecord>>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    Ok(Json(
        model::list(&ctx.db, auth_ctx.tenant_id, auth_ctx.workspace_id).await?,
    ))
}

async fn get(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(fork_id): Path<Uuid>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    Ok(Json(
        model::get(&ctx.db, auth_ctx.tenant_id, auth_ctx.workspace_id, fork_id).await?,
    ))
}

async fn create(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(input): Json<model::CreateInput>,
) -> Result<(StatusCode, Json<model::ForkRecord>), ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let txn = ctx.db.begin().await.internal()?;
    let fork = model::create(&txn, auth_ctx.tenant_id, auth_ctx.workspace_id, input).await?;
    txn.commit().await.internal()?;
    Ok((StatusCode::CREATED, Json(fork)))
}

async fn update(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(fork_id): Path<Uuid>,
    Json(input): Json<model::UpdateInput>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let txn = ctx.db.begin().await.internal()?;
    let fork = model::update(
        &txn,
        auth_ctx.tenant_id,
        auth_ctx.workspace_id,
        fork_id,
        input,
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(Json(fork))
}

async fn delete(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(fork_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let txn = ctx.db.begin().await.internal()?;
    model::delete(&txn, auth_ctx.tenant_id, auth_ctx.workspace_id, fork_id).await?;
    txn.commit().await.internal()?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/schema-forks")
        .add("", axum::routing::get(list).post(create))
        .add(
            "/{fork_id}",
            axum::routing::get(get).put(update).delete(delete),
        )
}
