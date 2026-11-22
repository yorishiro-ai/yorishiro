use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified};
use crate::controllers::openapi::Routes;
use crate::dtos::users::{UpdateUserRequest, UserRecord};
use crate::models::user_users;

async fn get_current_user(
    State(ctx): State<AppContext>,
    actor: Verified<ReadScope>,
) -> Result<Json<UserRecord>, ApiError> {
    Ok(Json(UserRecord::render(
        &ctx,
        user_users::current_profile(&ctx.db, &actor.ctx).await?,
    )))
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
    Routes::new()
        .tag("community")
        .prefix("api/users")
        .add(
            "/me",
            aide::axum::routing::get_with(get_current_user, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .response::<403, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/me",
            aide::axum::routing::put_with(update_current_user, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-scopes",
                    serde_json::json!(["read"]),
                ))
                .response::<403, Json<super::openapi::ApiErrorBody>>()
                .response::<409, Json<super::openapi::ApiErrorBody>>()
            }),
        )
}
