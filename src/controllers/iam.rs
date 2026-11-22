//! REST routes for IAM policies, groups, their attachments and the audit log.

use crate::controllers::Created;
use axum::{Json, extract::State};
use loco_rs::app::AppContext;

use crate::controllers::openapi::Routes;
use crate::controllers::{ApiError, extractors::AuthContext};
use crate::dtos::iam::{CreateGroup, CreatePolicy, GroupRecord, PolicyRecord};

async fn update_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::PolicyIdPath { policy_id: id }): axum::extract::Path<
        crate::dtos::common::PolicyIdPath<uuid::Uuid>,
    >,
    Json(input): Json<CreatePolicy>,
) -> Result<Json<PolicyRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_policies::Model::update_as(
            &ctx.db,
            &actor,
            id,
            input.name,
            input.document,
        )
        .await?
        .into(),
    ))
}

async fn delete_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::PolicyIdPath { policy_id: id }): axum::extract::Path<
        crate::dtos::common::PolicyIdPath<uuid::Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_policies::Model::delete_as(&ctx.db, &actor, id).await?;
    Ok(axum::response::NoContent)
}

async fn attach_user_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::UserIdPolicyIdPath { user_id, policy_id }): axum::extract::Path<crate::dtos::common::UserIdPolicyIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_user_policy_attachments::Model::add_as(&ctx.db, &actor, user_id, policy_id)
        .await?;
    Ok(axum::response::NoContent)
}

async fn detach_user_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::UserIdPolicyIdPath { user_id, policy_id }): axum::extract::Path<crate::dtos::common::UserIdPolicyIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_user_policy_attachments::Model::remove_as(
        &ctx.db, &actor, user_id, policy_id,
    )
    .await?;
    Ok(axum::response::NoContent)
}

async fn rename_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdPath { group_id: id }): axum::extract::Path<
        crate::dtos::common::GroupIdPath<uuid::Uuid>,
    >,
    Json(input): Json<CreateGroup>,
) -> Result<Json<GroupRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_groups::Model::rename_as(&ctx.db, &actor, id, input.name)
            .await?
            .into(),
    ))
}

async fn delete_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdPath { group_id: id }): axum::extract::Path<
        crate::dtos::common::GroupIdPath<uuid::Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_groups::Model::delete_as(&ctx.db, &actor, id).await?;
    Ok(axum::response::NoContent)
}

async fn list_audit(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListAudit>,
) -> Result<Json<Vec<crate::dtos::iam::AuditRecord>>, ApiError> {
    let events = crate::models::iam_audit_events::Model::list_as(
        &ctx.db,
        &actor,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(events.into_iter().map(Into::into).collect()))
}

async fn create_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreateGroup>,
) -> Result<Created<GroupRecord>, ApiError> {
    let group = crate::models::iam_groups::Model::create_as(&ctx.db, &actor, body.name).await?;
    Ok(Created(group.into()))
}

async fn create_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreatePolicy>,
) -> Result<Created<PolicyRecord>, ApiError> {
    let policy =
        crate::models::iam_policies::Model::create_as(&ctx.db, &actor, body.name, body.document)
            .await?;
    Ok(Created(policy.into()))
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/iam")
        .add(
            "/groups/{group_id}/policies",
            aide::axum::routing::get(list_iam_group_policies),
        )
        .add(
            "/users/{user_id}/policies",
            aide::axum::routing::get(list_iam_user_policies),
        )
        .add(
            "/groups/{group_id}/members",
            aide::axum::routing::get(list_iam_group_members),
        )
        .add(
            "/policies/{policy_id}",
            aide::axum::routing::put(update_policy),
        )
        .add(
            "/policies/{policy_id}",
            aide::axum::routing::delete(delete_policy),
        )
        .add(
            "/policies/{policy_id}",
            aide::axum::routing::get(get_iam_policy),
        )
        .add(
            "/groups/{group_id}/policies/{policy_id}",
            aide::axum::routing::put(attach_group_policy),
        )
        .add(
            "/groups/{group_id}/policies/{policy_id}",
            aide::axum::routing::delete(detach_group_policy),
        )
        .add(
            "/groups/{group_id}/members/{user_id}",
            aide::axum::routing::put(add_group_member),
        )
        .add(
            "/groups/{group_id}/members/{user_id}",
            aide::axum::routing::delete(remove_group_member),
        )
        .add(
            "/users/{user_id}/policies/{policy_id}",
            aide::axum::routing::put(attach_user_policy),
        )
        .add(
            "/users/{user_id}/policies/{policy_id}",
            aide::axum::routing::delete(detach_user_policy),
        )
        .add("/audit", aide::axum::routing::get(list_audit))
        .add("/groups", aide::axum::routing::post(create_group))
        .add("/groups", aide::axum::routing::get(list_iam_groups))
        .add("/groups/{group_id}", aide::axum::routing::put(rename_group))
        .add(
            "/groups/{group_id}",
            aide::axum::routing::delete(delete_group),
        )
        .add(
            "/groups/{group_id}",
            aide::axum::routing::get(get_iam_group),
        )
        .add("/policies", aide::axum::routing::post(create_policy))
        .add("/policies", aide::axum::routing::get(list_iam_policies))
}

async fn add_group_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdUserIdPath { group_id, user_id }): axum::extract::Path<crate::dtos::common::GroupIdUserIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_group_memberships::Model::add_as(&ctx.db, &actor, group_id, user_id).await?;
    Ok(axum::response::NoContent)
}

async fn remove_group_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdUserIdPath { group_id, user_id }): axum::extract::Path<crate::dtos::common::GroupIdUserIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_group_memberships::Model::remove_as(&ctx.db, &actor, group_id, user_id)
        .await?;
    Ok(axum::response::NoContent)
}

async fn attach_group_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdPolicyIdPath {
        group_id,
        policy_id,
    }): axum::extract::Path<crate::dtos::common::GroupIdPolicyIdPath<uuid::Uuid, uuid::Uuid>>,
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
    axum::extract::Path(crate::dtos::common::GroupIdPolicyIdPath {
        group_id,
        policy_id,
    }): axum::extract::Path<crate::dtos::common::GroupIdPolicyIdPath<uuid::Uuid, uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_group_policy_attachments::Model::remove_as(
        &ctx.db, &actor, group_id, policy_id,
    )
    .await?;
    Ok(axum::response::NoContent)
}

async fn list_iam_groups(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListIam>,
) -> Result<Json<Vec<GroupRecord>>, ApiError> {
    let rows = crate::models::iam_groups::Model::list_as(
        &ctx.db,
        &actor,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn list_iam_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListIam>,
) -> Result<Json<Vec<PolicyRecord>>, ApiError> {
    let rows = crate::models::iam_policies::Model::list_as(
        &ctx.db,
        &actor,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn list_iam_group_members(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdPath { group_id }): axum::extract::Path<
        crate::dtos::common::GroupIdPath<uuid::Uuid>,
    >,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListIam>,
) -> Result<Json<Vec<crate::dtos::iam::GroupMemberRecord>>, ApiError> {
    let rows = crate::models::iam_group_memberships::Model::list_as(
        &ctx.db,
        &actor,
        group_id,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn list_iam_user_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::UserIdPath { user_id }): axum::extract::Path<
        crate::dtos::common::UserIdPath<uuid::Uuid>,
    >,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListIam>,
) -> Result<Json<Vec<crate::dtos::iam::UserPolicyRecord>>, ApiError> {
    let rows = crate::models::iam_user_policy_attachments::Model::list_as(
        &ctx.db,
        &actor,
        user_id,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn list_iam_group_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdPath { group_id }): axum::extract::Path<
        crate::dtos::common::GroupIdPath<uuid::Uuid>,
    >,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListIam>,
) -> Result<Json<Vec<crate::dtos::iam::GroupPolicyRecord>>, ApiError> {
    let rows = crate::models::iam_group_policy_attachments::Model::list_as(
        &ctx.db,
        &actor,
        group_id,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn get_iam_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::GroupIdPath { group_id: id }): axum::extract::Path<
        crate::dtos::common::GroupIdPath<uuid::Uuid>,
    >,
) -> Result<Json<GroupRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_groups::Model::get_as(&ctx.db, &actor, id)
            .await?
            .into(),
    ))
}

async fn get_iam_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(crate::dtos::common::PolicyIdPath { policy_id: id }): axum::extract::Path<
        crate::dtos::common::PolicyIdPath<uuid::Uuid>,
    >,
) -> Result<Json<PolicyRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_policies::Model::get_as(&ctx.db, &actor, id)
            .await?
            .into(),
    ))
}
