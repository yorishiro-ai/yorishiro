use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(crate) struct OAuthStatus {
    pub(crate) enabled: bool,
}

#[derive(Deserialize)]
pub(crate) struct CallbackParams {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) error: Option<String>,
}
