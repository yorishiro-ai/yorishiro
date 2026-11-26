//! REST routes for users, their login profiles, group memberships and attached policies.

use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;

use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::controllers::{ApiError, Created};
use crate::dtos::common::{UserIdPath, UserIdPolicyIdPath};
use crate::dtos::groups::GroupRecord;
use crate::dtos::iam::ListIam;
use crate::dtos::policies::AttachedPolicyRecord;
use crate::dtos::users::{
    CreateUserRequest, UpdateLoginProfileRequest, UpdateUserRequest, UserRecord,
};
use crate::models::pagination::ListParams;

async fn list_users(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(input): Query<ListIam>,
) -> Result<Json<Vec<UserRecord>>, ApiError> {
    let users =
        crate::models::iam::users::list_as(&ctx.db, &actor, ListParams::from(&input)).await?;
    Ok(Json(
        users
            .into_iter()
            .map(|user| UserRecord::render(&ctx, actor.tenant_id, user))
            .collect(),
    ))
}

async fn create_user(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreateUserRequest>,
) -> Result<Created<UserRecord>, ApiError> {
    let user = crate::models::iam::users::create_as(
        &ctx.db,
        &actor,
        &body.user_name,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;
    Ok(Created(UserRecord::render(&ctx, actor.tenant_id, user)))
}

async fn get_user(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPath { user_id }): Path<UserIdPath<uuid::Uuid>>,
) -> Result<Json<UserRecord>, ApiError> {
    let user = crate::models::iam::users::get_as(&ctx.db, &actor, user_id).await?;
    Ok(Json(UserRecord::render(&ctx, actor.tenant_id, user)))
}

async fn update_user(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPath { user_id }): Path<UserIdPath<uuid::Uuid>>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<UserRecord>, ApiError> {
    let user =
        crate::models::iam::users::update_as(&ctx.db, &actor, user_id, body.display_name).await?;
    Ok(Json(UserRecord::render(&ctx, actor.tenant_id, user)))
}

async fn delete_user(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPath { user_id }): Path<UserIdPath<uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam::users::delete_as(&ctx.db, &actor, user_id).await?;
    Ok(axum::response::NoContent)
}

async fn update_login_profile(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPath { user_id }): Path<UserIdPath<uuid::Uuid>>,
    Json(body): Json<UpdateLoginProfileRequest>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam::users::update_login_profile_as(&ctx.db, &actor, user_id, &body.password)
        .await?;
    Ok(axum::response::NoContent)
}

async fn list_groups_for_user(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPath { user_id }): Path<UserIdPath<uuid::Uuid>>,
    Query(input): Query<ListIam>,
) -> Result<Json<Vec<GroupRecord>>, ApiError> {
    let groups = crate::models::iam_group_memberships::Model::groups_for_user_as(
        &ctx.db,
        &actor,
        user_id,
        ListParams::from(&input),
    )
    .await?;
    Ok(Json(groups.into_iter().map(Into::into).collect()))
}

async fn list_attached_user_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPath { user_id }): Path<UserIdPath<uuid::Uuid>>,
    Query(input): Query<ListIam>,
) -> Result<Json<Vec<AttachedPolicyRecord>>, ApiError> {
    let policies = crate::models::iam_user_policy_attachments::Model::list_as(
        &ctx.db,
        &actor,
        user_id,
        ListParams::from(&input),
    )
    .await?;
    Ok(Json(policies.into_iter().map(Into::into).collect()))
}

async fn attach_user_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPolicyIdPath { user_id, policy_id }): Path<
        UserIdPolicyIdPath<uuid::Uuid, uuid::Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_user_policy_attachments::Model::add_as(&ctx.db, &actor, user_id, policy_id)
        .await?;
    Ok(axum::response::NoContent)
}

async fn detach_user_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(UserIdPolicyIdPath { user_id, policy_id }): Path<
        UserIdPolicyIdPath<uuid::Uuid, uuid::Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_user_policy_attachments::Model::remove_as(
        &ctx.db, &actor, user_id, policy_id,
    )
    .await?;
    Ok(axum::response::NoContent)
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/users")
        .add(
            "/",
            aide::axum::routing::get_with(list_users, |op| op.id("ListUsers")),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_user, |op| op.id("CreateUser")),
        )
        .add(
            "/{user_id}",
            aide::axum::routing::get_with(get_user, |op| op.id("GetUser")),
        )
        .add(
            "/{user_id}",
            aide::axum::routing::patch_with(update_user, |op| op.id("UpdateUser")),
        )
        .add(
            "/{user_id}",
            aide::axum::routing::delete_with(delete_user, |op| op.id("DeleteUser")),
        )
        .add(
            "/{user_id}/login-profile",
            aide::axum::routing::put_with(update_login_profile, |op| op.id("UpdateLoginProfile")),
        )
        .add(
            "/{user_id}/groups",
            aide::axum::routing::get_with(list_groups_for_user, |op| op.id("ListGroupsForUser")),
        )
        .add(
            "/{user_id}/policies",
            aide::axum::routing::get_with(list_attached_user_policies, |op| {
                op.id("ListAttachedUserPolicies")
            }),
        )
        .add(
            "/{user_id}/policies/{policy_id}",
            aide::axum::routing::put_with(attach_user_policy, |op| op.id("AttachUserPolicy")),
        )
        .add(
            "/{user_id}/policies/{policy_id}",
            aide::axum::routing::delete_with(detach_user_policy, |op| op.id("DetachUserPolicy")),
        )
}
