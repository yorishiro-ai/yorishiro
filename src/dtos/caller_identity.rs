use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct CallerIdentityResponse {
    pub(crate) yrn: String,
    pub(crate) workspace_id: Option<Uuid>,
    pub(crate) tenant_id: Uuid,
    /// The user this key was issued for, if it is attributed to a user.
    pub(crate) user_id: Option<Uuid>,
    pub(crate) workspace_name: Option<String>,
    pub(crate) tenant_name: String,
    pub(crate) identity_kind: &'static str,
    /// Whether the credential is a tenant root sign-in session.
    pub(crate) tenant_root: bool,
}

impl From<crate::models::iam::access::AccessIdentity> for CallerIdentityResponse {
    fn from(identity: crate::models::iam::access::AccessIdentity) -> Self {
        let ctx = identity.current;
        Self {
            yrn: identity.yrn,
            workspace_name: identity.workspace_name,
            tenant_name: identity.tenant_name,
            tenant_root: identity.tenant_root,
            identity_kind: if ctx.user_id.is_some() {
                "user"
            } else {
                "service"
            },
            workspace_id: ctx.workspace_id,
            tenant_id: ctx.tenant_id,
            user_id: ctx.user_id,
        }
    }
}
