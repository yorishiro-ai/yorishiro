use crate::models::api_keys::ApiKeyScope;
use crate::models::tenant_memberships::MembershipRole;
use crate::models::workspace_workspaces::WorkspaceSummary;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// The workspaces the new member can now log into.
    /// The client picks one and passes its id to `/auth/login`.
    pub(crate) workspaces: Vec<WorkspaceSummary>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    /// Which of the account's workspaces to issue an API key for.
    /// Omit this when the account can only reach one workspace; it resolves automatically.
    /// An account reaching more than one receives `workspace_selection_required` (422) with typed candidates.
    pub workspace_id: Option<Uuid>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginResponse {
    /// The freshly issued API key's plaintext.
    /// Shown only in this response: only its hash is ever persisted, so it cannot be recovered afterward.
    pub api_key: String,
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Uuid,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LoginRecoveryCode {
    TenantMembershipRequired,
    WorkspaceProvisioningRequired,
    WorkspaceSelectionRequired,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginRecoveryResponse {
    pub error: LoginRecovery,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginRecovery {
    pub code: LoginRecoveryCode,
    pub message: String,
    pub hint: String,
    pub memberships: Vec<crate::models::tenancy::LoginMembership>,
    pub workspaces: Vec<crate::models::tenancy::LoginWorkspace>,
}
