use crate::models::workspace_workspaces::WorkspaceSummary;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SignupRequest {
    /// The email the new tenant root signs in with.
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SignupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    /// Workspaces currently accessible to the new tenant root.
    pub(crate) workspaces: Vec<WorkspaceSummary>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RootLoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct TenantLoginRequest {
    pub tenant_id: Uuid,
    pub user_name: String,
    pub password: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct LoginResponse {
    /// The freshly issued API key's plaintext.
    /// Shown only in this response: only its hash is ever persisted, so it cannot be recovered afterward.
    pub api_key: String,
    pub api_key_id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
}
