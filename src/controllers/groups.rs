//! REST routes for groups, their members and attached policies.

use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;

use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::controllers::{ApiError, Created};
use crate::dtos::common::{GroupIdPath, GroupIdPolicyIdPath, GroupIdUserIdPath};
use crate::dtos::groups::{CreateGroup, GroupDetail, GroupRecord};
use crate::dtos::iam::ListIam;
use crate::dtos::policies::AttachedPolicyRecord;
use crate::models::pagination::ListParams;

async fn list_groups(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(input): Query<ListIam>,
) -> Result<Json<Vec<GroupRecord>>, ApiError> {
    let rows = crate::models::iam_groups::Model::list_as(&ctx.db, &actor, ListParams::from(&input))
        .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn create_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreateGroup>,
) -> Result<Created<GroupRecord>, ApiError> {
    let group = crate::models::iam_groups::Model::create_as(&ctx.db, &actor, body.name).await?;
    Ok(Created(group.into()))
}

async fn get_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdPath { group_id }): Path<GroupIdPath<uuid::Uuid>>,
) -> Result<Json<GroupDetail>, ApiError> {
    Ok(Json(
        crate::models::iam_groups::Model::get_as(&ctx.db, &actor, group_id)
            .await?
            .into(),
    ))
}

async fn update_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdPath { group_id }): Path<GroupIdPath<uuid::Uuid>>,
    Json(body): Json<CreateGroup>,
) -> Result<Json<GroupRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_groups::Model::rename_as(&ctx.db, &actor, group_id, body.name)
            .await?
            .into(),
    ))
}

async fn delete_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdPath { group_id }): Path<GroupIdPath<uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_groups::Model::delete_as(&ctx.db, &actor, group_id).await?;
    Ok(axum::response::NoContent)
}

async fn add_user_to_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdUserIdPath { group_id, user_id }): Path<GroupIdUserIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_group_memberships::Model::add_as(&ctx.db, &actor, group_id, user_id).await?;
    Ok(axum::response::NoContent)
}

async fn remove_user_from_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdUserIdPath { group_id, user_id }): Path<GroupIdUserIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_group_memberships::Model::remove_as(&ctx.db, &actor, group_id, user_id)
        .await?;
    Ok(axum::response::NoContent)
}

async fn list_attached_group_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdPath { group_id }): Path<GroupIdPath<uuid::Uuid>>,
    Query(input): Query<ListIam>,
) -> Result<Json<Vec<AttachedPolicyRecord>>, ApiError> {
    let rows = crate::models::iam_group_policy_attachments::Model::list_as(
        &ctx.db,
        &actor,
        group_id,
        ListParams::from(&input),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn attach_group_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdPolicyIdPath {
        group_id,
        policy_id,
    }): Path<GroupIdPolicyIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_group_policy_attachments::Model::add_as(
        &ctx.db, &actor, group_id, policy_id,
    )
    .await?;
    Ok(axum::response::NoContent)
}

async fn detach_group_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(GroupIdPolicyIdPath {
        group_id,
        policy_id,
    }): Path<GroupIdPolicyIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_group_policy_attachments::Model::remove_as(
        &ctx.db, &actor, group_id, policy_id,
    )
    .await?;
    Ok(axum::response::NoContent)
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/groups")
        .add(
            "/",
            aide::axum::routing::get_with(list_groups, |op| op.id("ListGroups")),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_group, |op| op.id("CreateGroup")),
        )
        .add(
            "/{group_id}",
            aide::axum::routing::get_with(get_group, |op| op.id("GetGroup")),
        )
        .add(
            "/{group_id}",
            aide::axum::routing::patch_with(update_group, |op| op.id("UpdateGroup")),
        )
        .add(
            "/{group_id}",
            aide::axum::routing::delete_with(delete_group, |op| op.id("DeleteGroup")),
        )
        .add(
            "/{group_id}/members/{user_id}",
            aide::axum::routing::put_with(add_user_to_group, |op| op.id("AddUserToGroup")),
        )
        .add(
            "/{group_id}/members/{user_id}",
            aide::axum::routing::delete_with(remove_user_from_group, |op| {
                op.id("RemoveUserFromGroup")
            }),
        )
        .add(
            "/{group_id}/policies",
            aide::axum::routing::get_with(list_attached_group_policies, |op| {
                op.id("ListAttachedGroupPolicies")
            }),
        )
        .add(
            "/{group_id}/policies/{policy_id}",
            aide::axum::routing::put_with(attach_group_policy, |op| op.id("AttachGroupPolicy")),
        )
        .add(
            "/{group_id}/policies/{policy_id}",
            aide::axum::routing::delete_with(detach_group_policy, |op| op.id("DetachGroupPolicy")),
        )
}
