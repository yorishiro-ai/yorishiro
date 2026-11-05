use crate::models::api_keys::ApiKeyScope;
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct WhoAmIResponse {
    pub(crate) workspace_id: Uuid,
    pub(crate) tenant_id: Uuid,
    pub(crate) scope: ApiKeyScope,
    /// The user this key was issued for, if it was created with `admin create-api-key --user`.
    pub(crate) user_id: Option<Uuid>,
    /// Independent of `scope`: whether this key additionally holds the audit grant, checked separately by `GET /api/audit-log`.
    pub(crate) audit: bool,
    pub(crate) workspace_name: String,
    pub(crate) tenant_name: String,
    pub(crate) stored_scope: ApiKeyScope,
    /// None for a tenant-bound credential that selects a workspace per request.
    pub(crate) bound_workspace_id: Option<Uuid>,
    pub(crate) identity_kind: &'static str,
    pub(crate) workspace_access: crate::models::access::WorkspaceAccess,
}

impl From<crate::models::access::AccessIdentity> for WhoAmIResponse {
    fn from(identity: crate::models::access::AccessIdentity) -> Self {
        let ctx = identity.current;
        Self {
            workspace_name: identity.workspace_name,
            tenant_name: identity.tenant_name,
            stored_scope: identity.stored_scope,
            bound_workspace_id: identity.bound_workspace_id,
            workspace_access: identity.workspace_access,
            identity_kind: if ctx.user_id.is_some() {
                "user"
            } else {
                "service"
            },
            workspace_id: ctx.workspace_id,
            tenant_id: ctx.tenant_id,
            scope: ctx.scope,
            user_id: ctx.user_id,
            audit: ctx.audit,
        }
    }
}
