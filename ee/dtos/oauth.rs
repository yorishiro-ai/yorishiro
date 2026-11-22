use serde::{Deserialize, Serialize};

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct OAuthStatus {
    pub(crate) enabled: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CallbackParams {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) error: Option<String>,
}
