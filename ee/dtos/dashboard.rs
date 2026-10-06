use serde::Serialize;
use uuid::Uuid;

use crate::edition::ee::models::tenant_tenants::TenantUsage;
use crate::models::tenant_memberships::MembershipRecord;

#[derive(Debug, Serialize)]
pub(crate) struct TenantOverview {
    pub(crate) tenant_id: Uuid,
    /// `null` until a Stripe subscription event has set one.
    pub(crate) plan: Option<String>,
    pub(crate) max_workspaces: Option<i32>,
    pub(crate) usage: TenantUsage,
    pub(crate) members: Vec<MembershipRecord>,
}
