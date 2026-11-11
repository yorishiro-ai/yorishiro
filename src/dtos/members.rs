use crate::models::tenant_memberships::MembershipRole;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct PreviewRoleRequest {
    pub(crate) role: MembershipRole,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct TransferOwnershipRequest {
    pub(crate) successor_user_id: uuid::Uuid,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct AddMemberRequest {
    /// Must already have an account (created via `/auth/signup`): this endpoint attaches an *existing* user to the caller's tenant, it never creates one.
    /// To bring in someone with no account yet, issue them an invite instead.
    pub(crate) email: String,
    pub(crate) role: MembershipRole,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct CreateInviteRequest {
    pub(crate) email: String,
    pub(crate) role: MembershipRole,
    pub(crate) expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct ReissueInviteRequest {
    pub(crate) expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct InviteRecord {
    pub(crate) id: uuid::Uuid,
    pub(crate) tenant_id: uuid::Uuid,
    pub(crate) email: String,
    pub(crate) role: String,
    pub(crate) expires_at: chrono::DateTime<crate::time::Timezone>,
    pub(crate) used_at: Option<chrono::DateTime<crate::time::Timezone>>,
    pub(crate) created_at: chrono::DateTime<crate::time::Timezone>,
}

impl InviteRecord {
    pub(crate) fn render(
        ctx: &loco_rs::app::AppContext,
        model: crate::models::workspace_invites::Model,
    ) -> Self {
        Self {
            id: model.id,
            tenant_id: model.tenant_id,
            email: model.email,
            role: model.role,
            expires_at: crate::time::render(ctx, model.expires_at),
            used_at: model.used_at.map(|time| crate::time::render(ctx, time)),
            created_at: crate::time::render(ctx, model.created_at),
        }
    }
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct CreateInviteResponse {
    #[serde(flatten)]
    pub(crate) invite: InviteRecord,
    pub(crate) token: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct MemberAccessResponse {
    #[serde(flatten)]
    pub(crate) member: crate::models::tenant_memberships::MembershipRecord,
    /// Snapshot of existing attributed keys at the role update, not a key-issuance fence.
    pub(crate) keys: Vec<crate::models::tenant_memberships::MembershipKeyAccess>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct CreateTenantUserRequest {
    /// Unique within the tenant: 1 to 64 letters, digits, `.`, `_` or `-`.
    pub(crate) user_name: String,
    /// At least 12 characters.
    pub(crate) password: String,
    pub(crate) display_name: Option<String>,
    /// Admin, member or viewer; a tenant user cannot own the tenant.
    pub(crate) role: MembershipRole,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct ResetTenantUserPasswordRequest {
    /// At least 12 characters. Every key issued to the user is revoked.
    pub(crate) password: String,
}
