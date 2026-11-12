use crate::models::api_keys::ApiKeyScope;
use crate::models::tenant_memberships::MembershipRole;
use crate::models::workspace_workspaces::WorkspaceSummary;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AcceptInviteRequest {
    pub invite_token: String,
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AcceptAuthenticatedInviteRequest {
    pub invite_token: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AcceptInviteResponse {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub role: MembershipRole,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignupRequest {
    /// The plaintext token from an `admin create-invite`-issued invitation.
    /// Omit it to create a fresh tenant and join it as `Owner` instead.
    pub invite_token: Option<String>,
    /// Required when `invite_token` is omitted, rejected when it is present.
    pub email: Option<String>,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub role: MembershipRole,
    /// Workspaces currently accessible to the new member.
    pub(crate) workspaces: Vec<WorkspaceSummary>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootLoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TenantLoginRequest {
    pub user_name: String,
    pub password: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginResponse {
    /// The freshly issued API key's plaintext.
    /// Shown only in this response: only its hash is ever persisted, so it cannot be recovered afterward.
    pub api_key: String,
    pub api_key_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Uuid,
}
