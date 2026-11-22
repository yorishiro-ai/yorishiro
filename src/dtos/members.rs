use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTenantUserRequest {
    /// Unique within the tenant: 1 to 64 letters, digits, `.`, `_` or `-`.
    pub(crate) user_name: String,
    /// At least 12 characters.
    pub(crate) password: String,
    pub(crate) display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub(crate) struct ResetTenantUserPasswordRequest {
    /// At least 12 characters. Every key issued to the user is revoked.
    pub(crate) password: String,
}
