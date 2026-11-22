use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::models::tenant_memberships;
use crate::models::tenant_memberships::MembershipRecord;

async fn preview_removal(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::UserIdPath { user_id }): Path<
        crate::dtos::common::UserIdPath<uuid::Uuid>,
    >,
) -> Result<Json<tenant_memberships::MembershipRemovalPreview>, ApiError> {
    Ok(Json(
        tenant_memberships::preview_removal(&ctx.db, &actor, user_id).await?,
    ))
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

/// Creates a user bound to the caller's tenant, signed in with tenant id, user name and password.
///
/// # Errors
/// Returns an error for non-administrators, invalid credentials or a duplicate name.
async fn create_tenant_user(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Json(body): Json<crate::dtos::members::CreateTenantUserRequest>,
) -> Result<(StatusCode, Json<MembershipRecord>), ApiError> {
    let member = tenant_memberships::create_tenant_user_as(&ctx.db, &auth, &body).await?;
    Ok((StatusCode::CREATED, Json(member)))
}

/// Replaces a tenant user's password.
///
/// # Errors
/// Returns an error for non-administrators, a weak password or a user outside the tenant.
async fn reset_tenant_user_password(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::UserIdPath { user_id }): Path<
        crate::dtos::common::UserIdPath<uuid::Uuid>,
    >,
    Json(body): Json<crate::dtos::members::ResetTenantUserPasswordRequest>,
) -> Result<StatusCode, ApiError> {
    tenant_memberships::reset_tenant_user_password(&ctx.db, &auth, user_id, &body.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member(
    State(ctx): State<AppContext>,
    AuthContext(auth): AuthContext,
    Path(crate::dtos::common::UserIdPath { user_id }): Path<
        crate::dtos::common::UserIdPath<uuid::Uuid>,
    >,
) -> Result<StatusCode, ApiError> {
    tenant_memberships::remove_member_as(&ctx.db, &auth, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/members")
        .add("/", aide::axum::routing::get_with(list_members, |op| op.tag("community").security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-scopes", serde_json::json!(["read"]))).with(crate::controllers::openapi::extension("x-yorishiro-required-actions", serde_json::json!(["tenant_user_list"]))).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>()))
        .add("/users", aide::axum::routing::post_with(create_tenant_user, |op| op.tag("community").security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-scopes", serde_json::json!(["read"]))).with(crate::controllers::openapi::extension("x-yorishiro-required-actions", serde_json::json!(["tenant_user_create"]))).response::<201, Json<MembershipRecord>>().response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>().response::<409, Json<super::openapi::ApiErrorBody>>()))
        .add("/users/{user_id}/password", aide::axum::routing::put_with(reset_tenant_user_password, |op| op.tag("community").security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-scopes", serde_json::json!(["read"]))).with(crate::controllers::openapi::extension("x-yorishiro-required-actions", serde_json::json!(["tenant_user_password_reset"]))).response_with::<204, (), _>(|op| op.description("Password replaced and the user's keys revoked")).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>().response::<404, Json<super::openapi::ApiErrorBody>>().response::<409, Json<super::openapi::ApiErrorBody>>()))
        .add("/{user_id}", aide::axum::routing::delete_with(remove_member, |op| op.tag("community").security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-scopes", serde_json::json!(["read"]))).with(crate::controllers::openapi::extension("x-yorishiro-required-actions", serde_json::json!(["tenant_user_delete"]))).response_with::<204, (), _>(|op| op.description("Membership absent and attributed tenant keys revoked; a tenant user's account is deleted; repeated removal is safe")).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>().response::<409, Json<super::openapi::ApiErrorBody>>()))
        .add("/{user_id}/removal-preview", aide::axum::routing::get_with(preview_removal, |op| op.tag("community").security_requirement("bearer_auth").with(crate::controllers::openapi::extension("x-yorishiro-required-scopes", serde_json::json!(["read"]))).with(crate::controllers::openapi::extension("x-yorishiro-required-actions", serde_json::json!(["tenant_user_delete"]))).response::<401, Json<super::openapi::ApiErrorBody>>().response::<403, Json<super::openapi::ApiErrorBody>>()))
}
