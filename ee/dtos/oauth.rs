use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct OAuthStatus {
    pub(crate) enabled: bool,
}

#[derive(Deserialize)]
pub(crate) struct CallbackParams {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct AcceptOAuthInviteRequest {
    pub(crate) invite_token: String,
}
