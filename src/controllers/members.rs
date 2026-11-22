use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::models::tenant_memberships;
use crate::models::tenant_memberships::MembershipRecord;

#[cfg(feature = "openapi")]
fn preview_removal_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/members/{user_id}/removal-preview",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_user_delete"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<tenant_memberships::MembershipRemovalPreview>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn preview_removal(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(user_id): Path<uuid::Uuid>,
) -> Result<Json<tenant_memberships::MembershipRemovalPreview>, ApiError> {
    Ok(Json(
        tenant_memberships::preview_removal(&ctx.db, &actor, user_id).await?,
    ))
}

#[cfg(feature = "openapi")]
fn list_members_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/members",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_user_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<MembershipRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn list_members(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<MembershipRecord>>, ApiError> {
    let members = tenant_memberships::list_members_as(&ctx.db, &auth, page.try_into()?).await?;
    Ok(Json(members))
}

#[cfg(feature = "openapi")]
fn create_tenant_user_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/members/users",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::members::CreateTenantUserRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_user_create"]), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<MembershipRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
/// Creates a user bound to the caller's tenant, signed in with tenant id, user name and password.
///
/// # Errors
/// Returns an error for non-administrators, invalid credentials or a duplicate name.
async fn create_tenant_user(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<crate::dtos::members::CreateTenantUserRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let member = tenant_memberships::create_tenant_user_as(&ctx.db, &auth, &body).await?;
    Ok((StatusCode::CREATED, Json(member)))
}

#[cfg(feature = "openapi")]
fn reset_tenant_user_password_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/members/users/{user_id}/password",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::members::ResetTenantUserPasswordRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_user_password_reset"]), "responses": {"204": serde_json::json!({"description": "Password replaced and the user's keys revoked"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
/// Replaces a tenant user's password.
///
/// # Errors
/// Returns an error for non-administrators, a weak password or a user outside the tenant.
async fn reset_tenant_user_password(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(user_id): Path<uuid::Uuid>,
    Json(body): Json<crate::dtos::members::ResetTenantUserPasswordRequest>,
) -> Result<StatusCode, ApiError> {
    tenant_memberships::reset_tenant_user_password(&ctx.db, &auth, user_id, &body.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn remove_member_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/members/{user_id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["tenant_user_delete"]), "responses": {"204": serde_json::json!({"description": "Membership absent and attributed tenant keys revoked; a tenant user's account is deleted; repeated removal is safe"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "user_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn remove_member(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(user_id): Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    tenant_memberships::remove_member_as(&ctx.db, &auth, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        list_members_doc(),
        remove_member_doc(),
        create_tenant_user_doc(),
        reset_tenant_user_password_doc(),
        preview_removal_doc(),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/members")
        .add("/", get(list_members))
        .add("/users", post(create_tenant_user))
        .add("/users/{user_id}/password", put(reset_tenant_user_password))
        .add("/{user_id}", delete(remove_member))
        .add("/{user_id}/removal-preview", get(preview_removal))
}
