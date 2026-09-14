//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope};
use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::ee::models::workspace_schema_forks as model;

async fn list(authorized: Authorized<ReadScope>) -> Result<Json<Vec<model::ForkRecord>>, ApiError> {
    Ok(Json(
        model::list(
            authorized.txn(),
            authorized.ctx.tenant_id,
            authorized.ctx.workspace_id,
        )
        .await?,
    ))
}

async fn get(
    authorized: Authorized<ReadScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    Ok(Json(
        model::get(
            authorized.txn(),
            authorized.ctx.tenant_id,
            authorized.ctx.workspace_id,
            fork_id,
        )
        .await?,
    ))
}

async fn create(
    authorized: Authorized<SchemaScope>,
    Json(input): Json<model::CreateInput>,
) -> Result<(StatusCode, Json<model::ForkRecord>), ApiError> {
    let fork = model::create(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.workspace_id,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(fork)))
}

async fn update(
    authorized: Authorized<SchemaScope>,
    Path(fork_id): Path<Uuid>,
    Json(input): Json<model::UpdateInput>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let fork = model::update(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.workspace_id,
        fork_id,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(fork))
}

async fn delete(
    authorized: Authorized<SchemaScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    model::delete(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.workspace_id,
        fork_id,
    )
    .await?;
    authorized.commit().await?;
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
