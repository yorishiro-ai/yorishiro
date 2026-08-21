use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use utoipa::ToSchema;
use yorishiro_core::models::tenancy::{self, MembershipRecord, MembershipRole};
use yorishiro_core::{ResultExt, YorishiroError};

use crate::error::ApiError;
use crate::http::controllers::require_tenant_admin;
use crate::http::middleware::auth::AuthContext;
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/members",
    responses(
        (status = 200, description = "Members of the caller's tenant", body = Vec<MembershipRecord>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
    ),
    tag = "members",
)]
pub async fn list_members(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
) -> Result<Json<Vec<MembershipRecord>>, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;
    let members = tenancy::list_members(state.identity_pool()?, ctx.tenant_id).await?;
    Ok(Json(members))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddMemberRequest {
    /// Must already have an account (created via `/auth/signup`): this endpoint attaches an *existing* user to the caller's tenant, it never creates one.
    /// To bring in someone with no account yet, issue them an invite instead.
    pub email: String,
    pub role: MembershipRole,
}

#[utoipa::path(
    post,
    path = "/api/members",
    request_body = AddMemberRequest,
    responses(
        (status = 201, description = "Membership added (or role changed if already a member)", body = MembershipRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 404, description = "No user with this email has an account", body = crate::error::ApiErrorBody),
    ),
    tag = "members",
)]
pub async fn add_member(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Json(body): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;

    let mut conn = state.identity_pool()?.acquire().await.internal()?;
    let user = tenancy::get_user_by_email(&mut *conn, &body.email)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found(format!(
                "no user with email '{}' has an account",
                body.email
            ))
        })?;

    tenancy::add_member(&mut *conn, ctx.tenant_id, user.id, body.role).await?;

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

#[cfg(test)]
#[path = "../../../tests/http/controllers/members.rs"]
mod tests;
