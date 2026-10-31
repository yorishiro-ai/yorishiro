use serde::Serialize;
use uuid::Uuid;

use crate::edition::ee::models::tenant_tenants::TenantUsage;
use crate::models::tenant_memberships::MembershipRecord;

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct TenantOverview {
    pub(crate) tenant_id: Uuid,
    /// `null` until a Stripe subscription event has set one.
    pub(crate) plan: Option<String>,
    /// Currently enforced plan; null when plan policy is disabled.
    pub(crate) active_plan: Option<crate::edition::ee::data::plan::Plan>,
    /// Explicit operational limit, independent of plan policy.
    pub(crate) max_workspaces: Option<i32>,
    pub(crate) effective_max_workspaces: Option<i32>,
    pub(crate) usage: TenantUsage,
    pub(crate) members: Vec<MembershipRecord>,
}
