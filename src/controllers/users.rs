use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified};
use crate::dtos::users::{UpdateUserRequest, UserRecord};
use crate::models::user_users;

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/users/me", responses((status = 200, body = UserRecord), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "community"))]
async fn get_current_user(
    State(ctx): State<AppContext>,
    actor: Verified<ReadScope>,
) -> Result<Json<UserRecord>, ApiError> {
    Ok(Json(UserRecord::render(
        &ctx,
        user_users::current_profile(&ctx.db, &actor.ctx).await?,
    )))
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/users/me", request_body = UpdateUserRequest, responses((status = 200, body = UserRecord), (status = 403, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "community"))]
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

pub fn routes() -> Routes {
    Routes::new().prefix("api/users").add(
        "/me",
        axum::routing::get(get_current_user).put(update_current_user),
    )
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_get_current_user),
        super::route_inventory::path_doc(__path_update_current_user),
    ]
}
