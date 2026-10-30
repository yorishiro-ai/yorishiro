use crate::models::api_keys::ApiKeyScope;
use crate::time::Timezone;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateApiKeyRequest {
    pub(crate) name: String,
    pub(crate) scope: Option<ApiKeyScope>,
    pub(crate) expires_at: Option<DateTime<chrono::FixedOffset>>,
    #[serde(default)]
    pub(crate) audit: bool,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct ApiKeyRecord {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Timezone>,
    pub(crate) last_used_at: Option<DateTime<Timezone>>,
    #[serde(flatten)]
    pub(crate) grant: crate::models::api_keys::KeyGrant,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct CreateApiKeyResponse {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) full_key: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Timezone>,
    #[serde(flatten)]
    pub(crate) grant: crate::models::api_keys::KeyGrant,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct ListApiKeysResponse {
    pub(crate) keys: Vec<ApiKeyRecord>,
}
