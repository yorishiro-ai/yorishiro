use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified};
use crate::dtos::users::{UpdateUserRequest, UserRecord};
use crate::models::user_users;

#[cfg(feature = "openapi")]
fn get_current_user_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/users/me",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<UserRecord>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn get_current_user(
    State(ctx): State<AppContext>,
    actor: Verified<ReadScope>,
) -> Result<Json<UserRecord>, ApiError> {
    Ok(Json(UserRecord::render(
        &ctx,
        user_users::current_profile(&ctx.db, &actor.ctx).await?,
    )))
}

#[cfg(feature = "openapi")]
fn update_current_user_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/users/me",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<UpdateUserRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<UserRecord>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn update_current_user(
    State(ctx): State<AppContext>,
    actor: Verified<ReadScope>,
    Json(input): Json<UpdateUserRequest>,
) -> Result<Json<UserRecord>, ApiError> {
    Ok(Json(UserRecord::render(
        &ctx,
        user_users::update_profile(&ctx.db, &actor.ctx, input.display_name).await?,
    )))
}

pub(crate) fn routes() -> Routes {
    Routes::new().prefix("api/users").add(
        "/me",
        axum::routing::get(get_current_user).put(update_current_user),
    )
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![get_current_user_doc(), update_current_user_doc()]
}
