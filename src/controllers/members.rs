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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/members/{user_id}/removal-preview", params(("user_id" = uuid::Uuid, Path)), responses((status = 200, body = crate::models::tenant_memberships::MembershipRemovalPreview), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_delete"]))), tag = "community"))]
async fn preview_removal(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(user_id): Path<uuid::Uuid>,
) -> Result<Json<tenant_memberships::MembershipRemovalPreview>, ApiError> {
    Ok(Json(
        tenant_memberships::preview_removal(&ctx.db, &actor, user_id).await?,
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/members", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::models::tenant_memberships::MembershipRecord]), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_list"]))), tag = "community"))]
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

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/users", request_body = crate::dtos::members::CreateTenantUserRequest, responses((status = 201, body = MembershipRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_create"]))), tag = "community"))]
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

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/members/users/{user_id}/password", params(("user_id" = uuid::Uuid, Path)), request_body = crate::dtos::members::ResetTenantUserPasswordRequest, responses((status = 204, description = "Password replaced and the user's keys revoked"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_password_reset"]))), tag = "community"))]
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

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/members/{user_id}", params(("user_id" = uuid::Uuid, Path)), responses((status = 204, description = "Membership absent and attributed tenant keys revoked; a tenant user's account is deleted; repeated removal is safe"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_delete"]))), tag = "community"))]
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
        super::openapi::route_inventory::path_doc(__path_list_members),
        super::openapi::route_inventory::path_doc(__path_remove_member),
        super::openapi::route_inventory::path_doc(__path_create_tenant_user),
        super::openapi::route_inventory::path_doc(__path_reset_tenant_user_password),
        super::openapi::route_inventory::path_doc(__path_preview_removal),
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
