//! Which columns the Entities table shows, chosen per workspace and entity type.
//! Mounted at `/api/workspace/entity-columns` (singular): base's own workspace routes are mounted at `api/workspaces` (plural), so this does not collide.
//!
//! Reading needs `read`, writing needs `write`: this is a display preference, not schema state, so a key that may create entities may also decide how they are listed.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, WriteScope};
use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use loco_rs::controller::Routes;

use crate::edition::ee::dtos::entity_columns::SetColumnsRequest;
use crate::edition::ee::models::entity_column_preferences::{self, ColumnPreference};

/// `GET /api/workspace/entity-columns`
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/workspace/entity-columns", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::edition::ee::controllers::openapi::ColumnPreference]), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["entity_columns_get"]))), tag = "enterprise"))]
async fn list_columns(
    mut authorized: Authorized<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<ColumnPreference>>, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::EntityColumnsGet)
        .await?;
    let stored = entity_column_preferences::list(
        authorized.txn(),
        authorized.ctx.require_workspace()?,
        page.try_into()?,
    )
    .await?;
    Ok(Json(stored))
}

/// `PUT /api/workspace/entity-columns/{entity_type}`
#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/workspace/entity-columns/{entity_type}", params(("entity_type" = String, Path)), request_body = crate::edition::ee::controllers::openapi::SetColumnsRequest, responses((status = 200, body = crate::edition::ee::controllers::openapi::ColumnPreference), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["write"])), ("x-yorishiro-required-actions" = json!(["entity_columns_update"]))), tag = "enterprise"))]
async fn set_columns(
    mut authorized: Authorized<WriteScope>,
    Path(entity_type): Path<String>,
    Json(body): Json<SetColumnsRequest>,
) -> Result<Json<ColumnPreference>, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::EntityColumnsUpdate)
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
#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/workspace/entity-columns/{entity_type}", params(("entity_type" = String, Path)), responses((status = 204, description = "Column preferences reset"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 403, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["write"])), ("x-yorishiro-required-actions" = json!(["entity_columns_delete"]))), tag = "enterprise"))]
async fn reset_columns(
    mut authorized: Authorized<WriteScope>,
    Path(entity_type): Path<String>,
) -> Result<StatusCode, ApiError> {
    authorized
        .require_action(crate::models::iam_policy::Action::EntityColumnsDelete)
        .await?;
    entity_column_preferences::clear(
        authorized.txn(),
        authorized.ctx.require_workspace()?,
        &entity_type,
    )
    .await?;
    authorized.commit().await?;
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

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_list_columns),
        crate::controllers::route_inventory::path_doc(__path_set_columns),
        crate::controllers::route_inventory::path_doc(__path_reset_columns),
    ]
}
