//! Which columns the Entities table shows, chosen per workspace and entity type.
//!
//! Mounted at `/api/workspace/entity-columns` (singular): base's own workspace routes are mounted at `api/workspaces` (plural), so this does not collide.
//!
//! Reading needs `read`, writing needs `write`: this is a display preference, not schema state, so a key that may create entities may also decide how they are listed.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, Verified, WriteScope};
use crate::controllers::openapi::Routes;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;

use crate::edition::ee::dtos::entity_columns::SetColumnsRequest;
use crate::edition::ee::models::entity_column_preferences::{self, ColumnPreference};

/// `GET /api/workspace/entity-columns`
async fn list_columns(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<ColumnPreference>>, ApiError> {
    let stored =
        entity_column_preferences::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?;
    Ok(Json(stored))
}

/// `PUT /api/workspace/entity-columns/{entity_type}`
async fn set_columns(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(crate::dtos::common::EntityTypePath { entity_type }): Path<
        crate::dtos::common::EntityTypePath<String>,
    >,
    Json(body): Json<SetColumnsRequest>,
) -> Result<Json<ColumnPreference>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Permission(
        crate::models::iam::policy::Action::EntityColumnsUpdate,
    );
    let mut authorized = Authorized::<WriteScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::EntityColumnsUpdate)
        .await?;
    let stored = entity_column_preferences::set(
        authorized.txn(),
        authorized.ctx.require_workspace()?,
        &entity_type,
        &body.columns,
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(stored))
}

/// `DELETE /api/workspace/entity-columns/{entity_type}`
async fn reset_columns(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(crate::dtos::common::EntityTypePath { entity_type }): Path<
        crate::dtos::common::EntityTypePath<String>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Permission(
        crate::models::iam::policy::Action::EntityColumnsDelete,
    );
    let mut authorized = Authorized::<WriteScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::EntityColumnsDelete)
        .await?;
    entity_column_preferences::clear(
        authorized.txn(),
        authorized.ctx.require_workspace()?,
        &entity_type,
    )
    .await?;
    authorized.commit().await?;
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/workspace")
        .add(
            "/entity-columns",
            aide::axum::routing::get_with(list_columns, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["entity_columns_get"]),
                ))
            }),
        )
        .add(
            "/entity-columns/{entity_type}",
            aide::axum::routing::put_with(set_columns, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["entity_columns_update"]),
                ))
            }),
        )
        .add(
            "/entity-columns/{entity_type}",
            aide::axum::routing::delete_with(reset_columns, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["entity_columns_delete"]),
                ))
            }),
        )
}
