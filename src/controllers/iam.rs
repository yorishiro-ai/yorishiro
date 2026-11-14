use axum::{Json, extract::State, http::StatusCode, routing::post};
use loco_rs::{app::AppContext, controller::Routes};

use crate::controllers::{ApiError, extractors::AuthContext};
use crate::dtos::iam::{CreateGroup, CreatePolicy, GroupRecord, PolicyRecord};

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/iam/policies/{policy_id}", params(("policy_id" = uuid::Uuid, Path)), request_body = CreatePolicy, responses((status = 200, body = PolicyRecord), (status = 403), (status = 404), (status = 409), (status = 422)), security(("bearer_auth" = [])), tag = "community"))]
async fn update_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
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

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/iam/policies/{policy_id}", params(("policy_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404)), security(("bearer_auth" = [])), tag = "community"))]
async fn delete_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_policies::Model::delete_as(&ctx.db, &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/iam/users/{user_id}/policies/{policy_id}", params(("user_id" = uuid::Uuid, Path), ("policy_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404), (status = 409)), security(("bearer_auth" = [])), tag = "community"))]
async fn attach_user_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((user_id, policy_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_user_policy_attachments::Model::add_as(&ctx.db, &actor, user_id, policy_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/iam/users/{user_id}/policies/{policy_id}", params(("user_id" = uuid::Uuid, Path), ("policy_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404)), security(("bearer_auth" = [])), tag = "community"))]
async fn detach_user_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((user_id, policy_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_user_policy_attachments::Model::remove_as(
        &ctx.db, &actor, user_id, policy_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/iam/groups/{group_id}", params(("group_id" = uuid::Uuid, Path)), request_body = CreateGroup, responses((status = 200, body = GroupRecord), (status = 403), (status = 404)), security(("bearer_auth" = [])), tag = "community"))]
async fn rename_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(input): Json<CreateGroup>,
) -> Result<Json<GroupRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_groups::Model::rename_as(&ctx.db, &actor, id, input.name)
            .await?
            .into(),
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/iam/groups/{group_id}", params(("group_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404)), security(("bearer_auth" = [])), tag = "community"))]
async fn delete_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_groups::Model::delete_as(&ctx.db, &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/audit", params(("limit" = Option<i64>, Query), ("offset" = Option<i64>, Query)), responses((status = 200, body = Vec<crate::dtos::iam::AuditRecord>), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
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

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/iam/groups", request_body = CreateGroup, responses((status = 201, body = GroupRecord), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
async fn create_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreateGroup>,
) -> Result<(StatusCode, Json<GroupRecord>), ApiError> {
    let group = crate::models::iam_groups::Model::create_as(&ctx.db, &actor, body.name).await?;
    Ok((StatusCode::CREATED, Json(group.into())))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/iam/policies", request_body = CreatePolicy, responses((status = 201, body = PolicyRecord), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
async fn create_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreatePolicy>,
) -> Result<(StatusCode, Json<PolicyRecord>), ApiError> {
    let policy =
        crate::models::iam_policies::Model::create_as(&ctx.db, &actor, body.name, body.document)
            .await?;
    Ok((StatusCode::CREATED, Json(policy.into())))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_get_iam_policy),
        super::route_inventory::path_doc(__path_get_iam_group),
        super::route_inventory::path_doc(__path_list_iam_group_policies),
        super::route_inventory::path_doc(__path_list_iam_user_policies),
        super::route_inventory::path_doc(__path_list_iam_group_members),
        super::route_inventory::path_doc(__path_list_iam_policies),
        super::route_inventory::path_doc(__path_list_iam_groups),
        super::route_inventory::path_doc(__path_update_policy),
        super::route_inventory::path_doc(__path_delete_policy),
        super::route_inventory::path_doc(__path_detach_group_policy),
        super::route_inventory::path_doc(__path_attach_group_policy),
        super::route_inventory::path_doc(__path_remove_group_member),
        super::route_inventory::path_doc(__path_add_group_member),
        super::route_inventory::path_doc(__path_attach_user_policy),
        super::route_inventory::path_doc(__path_detach_user_policy),
        super::route_inventory::path_doc(__path_rename_group),
        super::route_inventory::path_doc(__path_delete_group),
        super::route_inventory::path_doc(__path_list_audit),
        super::route_inventory::path_doc(__path_create_group),
        super::route_inventory::path_doc(__path_create_policy),
    ]
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .prefix("api/iam")
        .add(
            "/groups/{group_id}/policies",
            axum::routing::get(list_iam_group_policies),
        )
        .add(
            "/users/{user_id}/policies",
            axum::routing::get(list_iam_user_policies),
        )
        .add(
            "/groups/{group_id}/members",
            axum::routing::get(list_iam_group_members),
        )
        .add(
            "/policies/{policy_id}",
            axum::routing::put(update_policy)
                .delete(delete_policy)
                .get(get_iam_policy),
        )
        .add(
            "/groups/{group_id}/policies/{policy_id}",
            axum::routing::put(attach_group_policy).delete(detach_group_policy),
        )
        .add(
            "/groups/{group_id}/members/{user_id}",
            axum::routing::put(add_group_member).delete(remove_group_member),
        )
        .add(
            "/users/{user_id}/policies/{policy_id}",
            axum::routing::put(attach_user_policy).delete(detach_user_policy),
        )
        .add("/audit", axum::routing::get(list_audit))
        .add("/groups", post(create_group).get(list_iam_groups))
        .add(
            "/groups/{group_id}",
            axum::routing::put(rename_group)
                .delete(delete_group)
                .get(get_iam_group),
        )
        .add("/policies", post(create_policy).get(list_iam_policies))
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/iam/groups/{group_id}/members/{user_id}", params(("group_id" = uuid::Uuid, Path), ("user_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404), (status = 409)), security(("bearer_auth" = [])), tag = "community"))]
async fn add_group_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((group_id, user_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_group_memberships::Model::add_as(&ctx.db, &actor, group_id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/iam/groups/{group_id}/members/{user_id}", params(("group_id" = uuid::Uuid, Path), ("user_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404), (status = 409)), security(("bearer_auth" = [])), tag = "community"))]
async fn remove_group_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((group_id, user_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_group_memberships::Model::remove_as(&ctx.db, &actor, group_id, user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/iam/groups/{group_id}/policies/{policy_id}", params(("group_id" = uuid::Uuid, Path), ("policy_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404), (status = 409)), security(("bearer_auth" = [])), tag = "community"))]
async fn attach_group_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((group_id, policy_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_group_policy_attachments::Model::add_as(
        &ctx.db, &actor, group_id, policy_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/iam/groups/{group_id}/policies/{policy_id}", params(("group_id" = uuid::Uuid, Path), ("policy_id" = uuid::Uuid, Path)), responses((status = 204), (status = 403), (status = 404), (status = 409)), security(("bearer_auth" = [])), tag = "community"))]
async fn detach_group_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((group_id, policy_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_group_policy_attachments::Model::remove_as(
        &ctx.db, &actor, group_id, policy_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/groups", params(("limit" = Option<i64>, Query), ("offset" = Option<i64>, Query)), responses((status = 200, body = Vec<crate::dtos::iam::GroupRecord>), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
async fn list_iam_groups(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListIam>,
) -> Result<Json<Vec<crate::dtos::iam::GroupRecord>>, ApiError> {
    let rows = crate::models::iam_groups::Model::list_as(
        &ctx.db,
        &actor,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/policies", params(("limit" = Option<i64>, Query), ("offset" = Option<i64>, Query)), responses((status = 200, body = Vec<crate::dtos::iam::PolicyRecord>), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
async fn list_iam_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Query(input): axum::extract::Query<crate::dtos::iam::ListIam>,
) -> Result<Json<Vec<crate::dtos::iam::PolicyRecord>>, ApiError> {
    let rows = crate::models::iam_policies::Model::list_as(
        &ctx.db,
        &actor,
        crate::models::pagination::ListParams::new(input.limit, input.offset),
    )
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/groups/{group_id}/members", params(("group_id" = uuid::Uuid, Path), ("limit" = Option<i64>, Query), ("offset" = Option<i64>, Query)), responses((status = 200, body = Vec<crate::dtos::iam::GroupMemberRecord>), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
async fn list_iam_group_members(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(group_id): axum::extract::Path<uuid::Uuid>,
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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/users/{user_id}/policies", params(("user_id" = uuid::Uuid, Path), ("limit" = Option<i64>, Query), ("offset" = Option<i64>, Query)), responses((status = 200, body = Vec<crate::dtos::iam::UserPolicyRecord>), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
async fn list_iam_user_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(user_id): axum::extract::Path<uuid::Uuid>,
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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/groups/{group_id}/policies", params(("group_id" = uuid::Uuid, Path), ("limit" = Option<i64>, Query), ("offset" = Option<i64>, Query)), responses((status = 200, body = Vec<crate::dtos::iam::GroupPolicyRecord>), (status = 403)), security(("bearer_auth" = [])), tag = "community"))]
async fn list_iam_group_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(group_id): axum::extract::Path<uuid::Uuid>,
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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/groups/{group_id}", params(("group_id" = uuid::Uuid, Path)), responses((status = 200, body = crate::dtos::iam::GroupRecord), (status = 403), (status = 404)), security(("bearer_auth" = [])), tag = "community"))]
async fn get_iam_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<crate::dtos::iam::GroupRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_groups::Model::get_as(&ctx.db, &actor, id)
            .await?
            .into(),
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/iam/policies/{policy_id}", params(("policy_id" = uuid::Uuid, Path)), responses((status = 200, body = crate::dtos::iam::PolicyRecord), (status = 403), (status = 404)), security(("bearer_auth" = [])), tag = "community"))]
async fn get_iam_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<crate::dtos::iam::PolicyRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_policies::Model::get_as(&ctx.db, &actor, id)
            .await?
            .into(),
    ))
}
