//! Which columns the Entities table shows, chosen per workspace and entity type.
//!
//! Mounted at `/api/workspace/entity-columns` (singular): base's own workspace routes are mounted at `api/workspaces` (plural), so this does not collide.
//!
//! Reading needs `read`, writing needs `write`: this is a display preference, not schema state, so a key that may create entities may also decide how they are listed.

use crate::controllers::ApiError;
use crate::controllers::extractors::Verified;
use crate::controllers::openapi::Routes;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;

use crate::edition::ee::dtos::entity_columns::SetColumnsRequest;
use crate::edition::ee::models::entity_column_preferences::{self, ColumnPreference};

/// `GET /api/workspace/entity-columns`
async fn list_columns(
    State(ctx): State<AppContext>,
    verified: Verified,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<ColumnPreference>>, ApiError> {
    let stored =
        entity_column_preferences::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?;
    Ok(Json(stored))
}

/// `PUT /api/workspace/entity-columns/{entity_type}`
async fn set_columns(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::EntityTypePath { entity_type }): Path<
        crate::dtos::common::EntityTypePath<String>,
    >,
    Json(body): Json<SetColumnsRequest>,
) -> Result<Json<ColumnPreference>, ApiError> {
    Ok(Json(
        entity_column_preferences::set_for_actor(&ctx, verified.ctx, &entity_type, &body.columns)
            .await?,
    ))
}

/// `DELETE /api/workspace/entity-columns/{entity_type}`
async fn reset_columns(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::EntityTypePath { entity_type }): Path<
        crate::dtos::common::EntityTypePath<String>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    entity_column_preferences::clear_for_actor(&ctx, verified.ctx, &entity_type).await?;
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/workspace")
        .add(
            "/entity-columns",
            aide::axum::routing::get_with(list_columns, |op| op.id("ListWorkspaceEntityColumns")),
        )
        .add(
            "/entity-columns/{entity_type}",
            aide::axum::routing::put_with(set_columns, |op| op.id("UpdateWorkspaceEntityColumns")),
        )
        .add(
            "/entity-columns/{entity_type}",
            aide::axum::routing::delete_with(reset_columns, |op| {
                op.id("DeleteWorkspaceEntityColumns")
            }),
        )
}
