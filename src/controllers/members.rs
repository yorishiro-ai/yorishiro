use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::dtos::members::AddMemberRequest;
use crate::error::YorishiroError;
use crate::models::tenant_memberships;
use crate::models::tenant_memberships::MembershipRecord;
use crate::models::user_users;

/// Shared by `members` and `workspaces`: both are tenant-wide concerns, independent of (and stricter than) the presented API key's own scope.
pub(crate) async fn require_tenant_admin(
    ctx: &AppContext,
    tenant_id: uuid::Uuid,
    user_id: Option<uuid::Uuid>,
) -> Result<(), YorishiroError> {
    let user_id = user_id.ok_or(YorishiroError::Unauthenticated)?;
    tenant_memberships::get_membership_role(&ctx.db, tenant_id, user_id)
        .await?
        .filter(|role| role.administers_tenant())
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this operation is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(())
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
    let members = tenant_memberships::list_members(&ctx.db, auth.tenant_id, page.into()).await?;
    Ok(Json(members))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/members", request_body = crate::dtos::members::AddMemberRequest, responses((status = 201, body = crate::models::tenant_memberships::MembershipRecord), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 404, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-roles" = json!(["tenant_admin"]))), tag = "community"))]
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

    tenant_memberships::add_member(&ctx.db, auth.tenant_id, user.id, body.role).await?;

    Ok((
        StatusCode::CREATED,
        Json(MembershipRecord {
            user_id: user.id,
            email: user.email,
            display_name: user.display_name,
            role: body.role,
        }),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_list_members),
        super::route_inventory::path_doc(__path_add_member),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/members")
        .add("/", get(list_members))
        .add("/", post(add_member))
}
