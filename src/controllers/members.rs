use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::dtos::members::{AddMemberRequest, MemberAccessResponse};
use crate::error::YorishiroError;
use crate::models::tenant_memberships;
use crate::models::tenant_memberships::MembershipRecord;
use crate::models::user_users;

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/members/{user_id}/removal-preview", params(("user_id" = uuid::Uuid, Path)), responses((status = 200, body = crate::models::tenant_memberships::MembershipRemovalPreview), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
async fn preview_removal(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(user_id): Path<uuid::Uuid>,
) -> Result<Json<tenant_memberships::MembershipRemovalPreview>, ApiError> {
    Ok(Json(
        tenant_memberships::preview_removal(&ctx.db, &actor, user_id).await?,
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/transfer-ownership", request_body = crate::dtos::members::TransferOwnershipRequest, responses((status = 204, description = "Successor promoted to owner and actor demoted to admin atomically"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_owner"]))), tag = "community"))]
async fn transfer_ownership(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<crate::dtos::members::TransferOwnershipRequest>,
) -> Result<StatusCode, ApiError> {
    tenant_memberships::transfer_ownership(&ctx.db, &actor, body.successor_user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/members/invites/{invite_id}", params(("invite_id" = uuid::Uuid, Path)), responses((status = 204, description = "Invite expired; repeated cancellation is safe"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
async fn revoke_invite(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(invite_id): Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    crate::models::workspace_invites::revoke_invite_as(&ctx.db, &auth, invite_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/invites/{invite_id}/reissue", params(("invite_id" = uuid::Uuid, Path)), request_body = crate::dtos::members::ReissueInviteRequest, responses((status = 200, body = crate::dtos::members::CreateInviteResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
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

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members/invites", request_body = crate::dtos::members::CreateInviteRequest, responses((status = 201, body = crate::dtos::members::CreateInviteResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
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

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/members/invites", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::dtos::members::InviteRecord]), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
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

/// Shared by `members` and `workspaces`: both are tenant-wide concerns, independent of (and stricter than) the presented API key's own scope.
pub(crate) async fn require_tenant_admin(
    ctx: &AppContext,
    tenant_id: uuid::Uuid,
    user_id: Option<uuid::Uuid>,
) -> Result<(), YorishiroError> {
    crate::models::access::require_tenant_admin(&ctx.db, tenant_id, user_id).await
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/members", params(("page" = Option<i32>, Query), ("page_size" = Option<i32>, Query)), responses((status = 200, body = [crate::models::tenant_memberships::MembershipRecord]), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn list_members(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<MembershipRecord>>, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    let members =
        tenant_memberships::list_members(&ctx.db, auth.tenant_id, page.try_into()?).await?;
    Ok(Json(members))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members", request_body = crate::dtos::members::AddMemberRequest, responses((status = 201, body = crate::dtos::members::MemberAccessResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn add_member(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let user = user_users::get_user_by_email(&ctx.db, &body.email)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found(format!(
                "no user with email '{}' has an account",
                body.email
            ))
        })?;

    let keys = tenant_memberships::update_member_access(&ctx.db, &auth, user.id, body.role).await?;

    Ok((
        StatusCode::CREATED,
        Json(MemberAccessResponse {
            member: MembershipRecord {
                user_id: user.id,
                email: user.email,
                display_name: user.display_name,
                role: body.role,
            },
            keys,
        }),
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/members/{user_id}", params(("user_id" = uuid::Uuid, Path)), responses((status = 204, description = "Membership absent and attributed tenant keys revoked; repeated removal is safe"), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
async fn remove_member(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(user_id): Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
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
        super::route_inventory::path_doc(__path_preview_removal),
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
        .add("/{user_id}", delete(remove_member))
        .add("/{user_id}/removal-preview", get(preview_removal))
        .add("/invites", post(create_invite))
        .add("/invites", get(list_invites))
        .add("/invites/{invite_id}", delete(revoke_invite))
        .add("/invites/{invite_id}/reissue", post(reissue_invite))
}
