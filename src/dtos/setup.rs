use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct SetupStatusResponse {
    /// True when the wizard is enabled and no tenant exists yet: the client should show the setup form instead of the login form.
    pub(crate) setup_required: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SetupRequest {
    pub(crate) email: String,
    pub(crate) password: String,
    pub(crate) display_name: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct SetupResponse {
    pub(crate) user_id: Uuid,
    pub(crate) email: String,
    pub(crate) tenant_id: Uuid,
    /// A freshly issued access key for the new admin account.
    /// Shown only here, so the setup screen can sign straight in.
    pub(crate) access_key: String,
}
