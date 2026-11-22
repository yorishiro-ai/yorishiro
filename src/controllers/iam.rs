//! REST routes for IAM policies, groups, their attachments and the audit log.

use axum::{Json, extract::State, http::StatusCode, routing::post};
use loco_rs::{app::AppContext, controller::Routes};

use crate::controllers::{ApiError, extractors::AuthContext};
use crate::dtos::iam::{CreateGroup, CreatePolicy, GroupRecord, PolicyRecord};

#[cfg(feature = "openapi")]
fn update_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/policies/{policy_id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreatePolicy>()}}}, "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<PolicyRecord>()}}}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""}), "409": serde_json::json!({"description": ""}), "422": serde_json::json!({"description": ""})}, "parameters": [{"name": "policy_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn delete_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/policies/{policy_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""})}, "parameters": [{"name": "policy_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn delete_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_policies::Model::delete_as(&ctx.db, &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn attach_user_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/users/{user_id}/policies/{policy_id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""}), "409": serde_json::json!({"description": ""})}, "parameters": [{"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "policy_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn attach_user_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((user_id, policy_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_user_policy_attachments::Model::add_as(&ctx.db, &actor, user_id, policy_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn detach_user_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/users/{user_id}/policies/{policy_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""})}, "parameters": [{"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "policy_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn rename_group_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateGroup>()}}}, "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<GroupRecord>()}}}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn delete_group_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn delete_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_groups::Model::delete_as(&ctx.db, &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn list_audit_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/audit",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<crate::dtos::iam::AuditRecord>>()}}}), "403": serde_json::json!({"description": ""})}, "parameters": [{"name": "limit", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}, {"name": "offset", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}]}),
    )
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

#[cfg(feature = "openapi")]
fn create_group_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateGroup>()}}}, "security": [{"bearer_auth": []}], "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<GroupRecord>()}}}), "403": serde_json::json!({"description": ""})}}),
    )
}
async fn create_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreateGroup>,
) -> Result<(StatusCode, Json<GroupRecord>), ApiError> {
    let group = crate::models::iam_groups::Model::create_as(&ctx.db, &actor, body.name).await?;
    Ok((StatusCode::CREATED, Json(group.into())))
}

#[cfg(feature = "openapi")]
fn create_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/policies",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreatePolicy>()}}}, "security": [{"bearer_auth": []}], "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<PolicyRecord>()}}}), "403": serde_json::json!({"description": ""})}}),
    )
}
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
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        get_iam_policy_doc(),
        get_iam_group_doc(),
        list_iam_group_policies_doc(),
        list_iam_user_policies_doc(),
        list_iam_group_members_doc(),
        list_iam_policies_doc(),
        list_iam_groups_doc(),
        update_policy_doc(),
        delete_policy_doc(),
        detach_group_policy_doc(),
        attach_group_policy_doc(),
        remove_group_member_doc(),
        add_group_member_doc(),
        attach_user_policy_doc(),
        detach_user_policy_doc(),
        rename_group_doc(),
        delete_group_doc(),
        list_audit_doc(),
        create_group_doc(),
        create_policy_doc(),
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

#[cfg(feature = "openapi")]
fn add_group_member_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}/members/{user_id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""}), "409": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn add_group_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((group_id, user_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_group_memberships::Model::add_as(&ctx.db, &actor, group_id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn remove_group_member_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}/members/{user_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""}), "409": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn remove_group_member(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path((group_id, user_id)): axum::extract::Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    crate::models::iam_group_memberships::Model::remove_as(&ctx.db, &actor, group_id, user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn attach_group_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}/policies/{policy_id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""}), "409": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "policy_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn detach_group_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}/policies/{policy_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"204": serde_json::json!({"description": ""}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""}), "409": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "policy_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn list_iam_groups_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<GroupRecord>>()}}}), "403": serde_json::json!({"description": ""})}, "parameters": [{"name": "limit", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}, {"name": "offset", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}]}),
    )
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

#[cfg(feature = "openapi")]
fn list_iam_policies_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/policies",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<PolicyRecord>>()}}}), "403": serde_json::json!({"description": ""})}, "parameters": [{"name": "limit", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}, {"name": "offset", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}]}),
    )
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

#[cfg(feature = "openapi")]
fn list_iam_group_members_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}/members",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<crate::dtos::iam::GroupMemberRecord>>()}}}), "403": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "limit", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}, {"name": "offset", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn list_iam_user_policies_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/users/{user_id}/policies",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<crate::dtos::iam::UserPolicyRecord>>()}}}), "403": serde_json::json!({"description": ""})}, "parameters": [{"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "limit", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}, {"name": "offset", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn list_iam_group_policies_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}/policies",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<crate::dtos::iam::GroupPolicyRecord>>()}}}), "403": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "limit", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}, {"name": "offset", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i64>()}]}),
    )
}
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

#[cfg(feature = "openapi")]
fn get_iam_group_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/groups/{group_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<GroupRecord>()}}}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""})}, "parameters": [{"name": "group_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn get_iam_group(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<GroupRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_groups::Model::get_as(&ctx.db, &actor, id)
            .await?
            .into(),
    ))
}

#[cfg(feature = "openapi")]
fn get_iam_policy_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/iam/policies/{policy_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<PolicyRecord>()}}}), "403": serde_json::json!({"description": ""}), "404": serde_json::json!({"description": ""})}, "parameters": [{"name": "policy_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn get_iam_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<PolicyRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_policies::Model::get_as(&ctx.db, &actor, id)
            .await?
            .into(),
    ))
}
