//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::controllers::Created;
use crate::controllers::extractors::Verified;
use crate::controllers::openapi::Routes;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::edition::ee::models::workspace_schema_forks as model;

async fn list(
    State(ctx): State<AppContext>,
    verified: Verified,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<model::ForkRecord>>, ApiError> {
    Ok(Json(
        model::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?,
    ))
}

async fn get(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::ForkIdPath { fork_id }): Path<crate::dtos::common::ForkIdPath<Uuid>>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    Ok(Json(
        model::get_for_actor(&ctx, verified.ctx, fork_id).await?,
    ))
}

async fn create(
    State(ctx): State<AppContext>,
    verified: Verified,
    Json(input): Json<model::CreateInput>,
) -> Result<Created<model::ForkRecord>, ApiError> {
    Ok(Created(
        model::create_for_actor(&ctx, verified.ctx, input).await?,
    ))
}

async fn update(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::ForkIdPath { fork_id }): Path<crate::dtos::common::ForkIdPath<Uuid>>,
    Json(input): Json<model::UpdateInput>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    Ok(Json(
        model::update_for_actor(&ctx, verified.ctx, fork_id, input).await?,
    ))
}

async fn delete(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::ForkIdPath { fork_id }): Path<crate::dtos::common::ForkIdPath<Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    model::delete_for_actor(&ctx, verified.ctx, fork_id).await?;
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/schema-forks")
        .add(
            "",
            aide::axum::routing::get_with(list, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_list"]),
                ))
            }),
        )
        .add(
            "",
            aide::axum::routing::post_with(create, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_create"]),
                ))
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::get_with(get, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_get"]),
                ))
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::put_with(update, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_update"]),
                ))
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::delete_with(delete, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_delete"]),
                ))
            }),
        )
}
