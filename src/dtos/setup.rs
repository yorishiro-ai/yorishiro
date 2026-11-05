use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct SetupStatusResponse {
    /// True when the wizard is enabled and no tenant exists yet: the client should show the setup form instead of the login form.
    pub(crate) setup_required: bool,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct SetupRequest {
    pub(crate) email: String,
    pub(crate) password: String,
    pub(crate) display_name: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct SetupResponse {
    pub(crate) user_id: Uuid,
    pub(crate) email: String,
    pub(crate) tenant_id: Uuid,
    /// The default workspace created for the tenant.
    /// The returned key is not bound to it: it is tenant-scoped and names its workspace per request.
    pub(crate) workspace_id: Uuid,
    /// A freshly issued tenant-scoped admin key, scoped to the new owner account: shown only here, same as `/auth/login`'s, so the setup screen can log straight into the dashboard afterward.
    pub(crate) api_key: String,
}
