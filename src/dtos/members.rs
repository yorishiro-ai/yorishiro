use crate::models::tenant_memberships::MembershipRole;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AddMemberRequest {
    /// Must already have an account (created via `/auth/signup`): this endpoint attaches an *existing* user to the caller's tenant, it never creates one.
    /// To bring in someone with no account yet, issue them an invite instead.
    pub email: String,
    pub role: MembershipRole,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MemberAccessResponse {
    #[serde(flatten)]
    pub member: crate::models::tenant_memberships::MembershipRecord,
    /// Snapshot of existing attributed keys at the role update, not a key-issuance fence.
    pub keys: Vec<crate::models::tenant_memberships::MembershipKeyAccess>,
}
