use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct WhoAmIResponse {
    pub(crate) workspace_id: Option<Uuid>,
    pub(crate) tenant_id: Uuid,
    /// The user this key was issued for, if it is attributed to a user.
    pub(crate) user_id: Option<Uuid>,
    /// Whether this key holds the audit grant, which `GET /api/audit-log` requires.
    pub(crate) audit: bool,
    pub(crate) workspace_name: Option<String>,
    pub(crate) tenant_name: String,
    /// None for a tenant-bound credential that selects a workspace per request.
    pub(crate) bound_workspace_id: Option<Uuid>,
    pub(crate) identity_kind: &'static str,
    /// Root identity does not bypass credential binding or audit requirements.
    pub(crate) tenant_root: bool,
}

impl From<crate::models::iam::access::AccessIdentity> for WhoAmIResponse {
    fn from(identity: crate::models::iam::access::AccessIdentity) -> Self {
        let ctx = identity.current;
        Self {
            workspace_name: identity.workspace_name,
            tenant_name: identity.tenant_name,
            bound_workspace_id: identity.bound_workspace_id,
            tenant_root: identity.tenant_root,
            identity_kind: if ctx.user_id.is_some() {
                "user"
            } else {
                "service"
            },
            workspace_id: ctx.workspace_id,
            tenant_id: ctx.tenant_id,
            user_id: ctx.user_id,
            audit: ctx.audit,
        }
    }
}
