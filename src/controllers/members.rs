use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::dtos::members::{AddMemberRequest, MemberAccessResponse};
use crate::error::ResultExt;
use crate::models::tenant_memberships;
use crate::models::tenant_memberships::MembershipRecord;

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/{user_id}/role-preview", params(("user_id" = uuid::Uuid, Path)), request_body = crate::dtos::members::PreviewRoleRequest, responses((status = 200, body = crate::models::tenant_memberships::MembershipRolePreview), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_update"]))), tag = "community"))]
async fn preview_role_change(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(user_id): Path<uuid::Uuid>,
    Json(body): Json<crate::dtos::members::PreviewRoleRequest>,
) -> Result<Json<tenant_memberships::MembershipRolePreview>, ApiError> {
    Ok(Json(
        tenant_memberships::preview_role_change(&ctx.db, &actor, user_id, body.role).await?,
    ))
}

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

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/transfer-ownership", request_body = crate::dtos::members::TransferOwnershipRequest, responses((status = 204, description = "Successor promoted to owner and actor demoted to admin atomically"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_update"]))), tag = "community"))]
async fn transfer_ownership(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<crate::dtos::members::TransferOwnershipRequest>,
) -> Result<StatusCode, ApiError> {
    tenant_memberships::transfer_ownership(&ctx.db, &actor, body.successor_user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/members/invites/{invite_id}", params(("invite_id" = uuid::Uuid, Path)), responses((status = 204, description = "Invite expired; repeated cancellation is safe"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_delete"]))), tag = "community"))]
async fn revoke_invite(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(invite_id): Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    crate::models::workspace_invites::revoke_invite_as(&ctx.db, &auth, invite_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/invites/{invite_id}/reissue", params(("invite_id" = uuid::Uuid, Path)), request_body = crate::dtos::members::ReissueInviteRequest, responses((status = 200, body = crate::dtos::members::CreateInviteResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_update"]))), tag = "community"))]
async fn reissue_invite(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(invite_id): Path<uuid::Uuid>,
    Json(body): Json<crate::dtos::members::ReissueInviteRequest>,
) -> Result<Json<crate::dtos::members::CreateInviteResponse>, ApiError> {
    let (invite, token) = crate::models::workspace_invites::reissue_invite_as(
        &ctx.db,
        &auth,
        invite_id,
        body.expires_at,
    )
    .await?;
    Ok(Json(crate::dtos::members::CreateInviteResponse {
        invite: crate::dtos::members::InviteRecord::render(&ctx, invite),
        token,
    }))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/invites", request_body = crate::dtos::members::CreateInviteRequest, responses((status = 201, body = crate::dtos::members::CreateInviteResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_create"]))), tag = "community"))]
async fn create_invite(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<crate::dtos::members::CreateInviteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (invite, token) = crate::models::workspace_invites::create_invite_as(
        &ctx.db,
        &auth,
        &body.email,
        body.role,
        body.expires_at,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(crate::dtos::members::CreateInviteResponse {
            invite: crate::dtos::members::InviteRecord::render(&ctx, invite),
            token,
        }),
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/members/invites", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::dtos::members::InviteRecord]), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_list"]))), tag = "community"))]
async fn list_invites(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<crate::dtos::members::InviteRecord>>, ApiError> {
    let invites =
        crate::models::workspace_invites::list_invites(&ctx.db, &auth, page.try_into()?).await?;
    Ok(Json(
        invites
            .into_iter()
            .map(|invite| crate::dtos::members::InviteRecord::render(&ctx, invite))
            .collect(),
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

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members", request_body = crate::dtos::members::AddMemberRequest, responses((status = 201, body = crate::dtos::members::MemberAccessResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["tenant_user_update"]))), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn add_member(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (user, keys) =
        tenant_memberships::update_member_by_email_as(&ctx.db, &auth, &body.email, body.role)
            .await?;

    Ok((
        StatusCode::CREATED,
        Json(MemberAccessResponse {
            member: MembershipRecord {
                user_id: user.id,
                email: user.email,
                user_name: user.user_name,
                display_name: user.display_name,
                role: body.role,
            },
            keys,
        }),
    ))
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
    let txn = crate::db::begin_write(&ctx.db).await.internal()?;
    crate::models::access::require_iam_action(
        &txn,
        &auth,
        crate::models::api_keys::ApiKeyScope::Read,
        crate::models::iam_policy::Action::TenantUserCreate,
        crate::models::iam_policy::Resource::Tenant {
            tenant_id: auth.tenant_id,
        },
    )
    .await?;
    let user = tenant_memberships::create_tenant_user_member(
        &txn,
        auth.tenant_id,
        &body.user_name,
        &body.password,
        body.display_name.as_deref(),
        body.role,
    )
    .await?;
    txn.commit().await.internal()?;
    Ok((
        StatusCode::CREATED,
        Json(MembershipRecord {
            user_id: user.id,
            email: None,
            user_name: user.user_name,
            display_name: user.display_name,
            role: body.role,
        }),
    ))
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
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_list_members),
        super::route_inventory::path_doc(__path_transfer_ownership),
        super::route_inventory::path_doc(__path_add_member),
        super::route_inventory::path_doc(__path_remove_member),
        super::route_inventory::path_doc(__path_create_tenant_user),
        super::route_inventory::path_doc(__path_reset_tenant_user_password),
        super::route_inventory::path_doc(__path_preview_removal),
        super::route_inventory::path_doc(__path_preview_role_change),
        super::route_inventory::path_doc(__path_create_invite),
        super::route_inventory::path_doc(__path_list_invites),
        super::route_inventory::path_doc(__path_revoke_invite),
        super::route_inventory::path_doc(__path_reissue_invite),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/members")
        .add("/", get(list_members))
        .add("/transfer-ownership", post(transfer_ownership))
        .add("/", post(add_member))
        .add("/users", post(create_tenant_user))
        .add("/users/{user_id}/password", put(reset_tenant_user_password))
        .add("/{user_id}", delete(remove_member))
        .add("/{user_id}/removal-preview", get(preview_removal))
        .add("/{user_id}/role-preview", post(preview_role_change))
        .add("/invites", post(create_invite))
        .add("/invites", get(list_invites))
        .add("/invites/{invite_id}", delete(revoke_invite))
        .add("/invites/{invite_id}/reissue", post(reissue_invite))
}
