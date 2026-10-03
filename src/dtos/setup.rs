use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
pub struct SetupStatusResponse {
    /// True when the wizard is enabled and no tenant exists yet: the client should show the setup form instead of the login form.
    pub setup_required: bool,
}

#[derive(Deserialize)]
pub struct SetupRequest {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Serialize)]
pub struct SetupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    /// A freshly issued API key, scoped to the new owner account: shown only here, same as `/auth/login`'s, so the setup screen can log straight into the dashboard afterward.
    pub api_key: String,
}
