use crate::models::tenant_memberships::MembershipRole;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TransferOwnershipRequest {
    pub successor_user_id: uuid::Uuid,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AddMemberRequest {
    /// Must already have an account (created via `/auth/signup`): this endpoint attaches an *existing* user to the caller's tenant, it never creates one.
    /// To bring in someone with no account yet, issue them an invite instead.
    pub email: String,
    pub role: MembershipRole,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateInviteRequest {
    pub email: String,
    pub role: MembershipRole,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReissueInviteRequest {
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InviteRecord {
    pub id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub email: String,
    pub role: String,
    pub expires_at: chrono::DateTime<crate::time::Timezone>,
    pub used_at: Option<chrono::DateTime<crate::time::Timezone>>,
    pub created_at: chrono::DateTime<crate::time::Timezone>,
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
pub struct CreateInviteResponse {
    #[serde(flatten)]
    pub invite: InviteRecord,
    pub token: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MemberAccessResponse {
    #[serde(flatten)]
    pub member: crate::models::tenant_memberships::MembershipRecord,
    /// Snapshot of existing attributed keys at the role update, not a key-issuance fence.
    pub keys: Vec<crate::models::tenant_memberships::MembershipKeyAccess>,
}
