use crate::models::tenant_memberships::MembershipRole;
use serde::Deserialize;

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AddMemberRequest {
    /// Must already have an account (created via `/auth/signup`): this endpoint attaches an *existing* user to the caller's tenant, it never creates one.
    /// To bring in someone with no account yet, issue them an invite instead.
    pub email: String,
    pub role: MembershipRole,
}
