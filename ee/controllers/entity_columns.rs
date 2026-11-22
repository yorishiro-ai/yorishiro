//! Which columns the Entities table shows, chosen per workspace and entity type.
//!
//! Mounted at `/api/workspace/entity-columns` (singular): base's own workspace routes are mounted at `api/workspaces` (plural), so this does not collide.
//!
//! Reading needs `read`, writing needs `write`: this is a display preference, not schema state, so a key that may create entities may also decide how they are listed.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, Verified, WriteScope};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::edition::ee::dtos::entity_columns::SetColumnsRequest;
use crate::edition::ee::models::entity_column_preferences::{self, ColumnPreference};

/// `GET /api/workspace/entity-columns`
#[cfg(feature = "openapi")]
fn list_columns_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/entity-columns",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_columns_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<ColumnPreference>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
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
#[cfg(feature = "openapi")]
fn set_columns_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/entity-columns/{entity_type}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<SetColumnsRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_columns_update"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<ColumnPreference>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "entity_type", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn set_columns(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(entity_type): Path<String>,
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
#[cfg(feature = "openapi")]
fn reset_columns_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/entity-columns/{entity_type}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_columns_delete"]), "responses": {"204": serde_json::json!({"description": "Column preferences reset"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "entity_type", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn reset_columns(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(entity_type): Path<String>,
) -> Result<StatusCode, ApiError> {
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
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![list_columns_doc(), set_columns_doc(), reset_columns_doc()]
}
