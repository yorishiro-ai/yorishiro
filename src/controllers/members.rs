use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::Deserialize;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::error::YorishiroError;
use crate::models::tenancy::{self, MembershipRecord, MembershipRole};

/// Shared by `members` and `workspaces`: both are tenant-wide concerns, independent of (and stricter than) the presented API key's own scope.
pub(crate) async fn require_tenant_admin(
    ctx: &AppContext,
    tenant_id: uuid::Uuid,
    user_id: Option<uuid::Uuid>,
) -> Result<(), YorishiroError> {
    let user_id = user_id.ok_or(YorishiroError::Unauthenticated)?;
    tenancy::get_membership_role(&ctx.db, tenant_id, user_id)
        .await?
        .filter(|role| role.administers_tenant())
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this operation is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(())
}

pub async fn list_members(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
) -> Result<Json<Vec<MembershipRecord>>, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;
    let members = tenancy::list_members(&ctx.db, auth.tenant_id).await?;
    Ok(Json(members))
}

#[derive(Debug, Deserialize)]
pub struct AddMemberRequest {
    /// Must already have an account (created via `/auth/signup`): this endpoint attaches an *existing* user to the caller's tenant, it never creates one.
    /// To bring in someone with no account yet, issue them an invite instead.
    pub email: String,
    pub role: MembershipRole,
}

pub async fn add_member(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&ctx, auth.tenant_id, auth.user_id).await?;

    let user = tenancy::get_user_by_email(&ctx.db, &body.email)
        .await?
        .ok_or_else(|| {
            YorishiroError::not_found(format!(
                "no user with email '{}' has an account",
                body.email
            ))
        })?;

    tenancy::add_member(&ctx.db, auth.tenant_id, user.id, body.role).await?;

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

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/members")
        .add("/", get(list_members))
        .add("/", post(add_member))
}
