use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct CreateApiKeyRequest {
    pub(crate) name: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct ApiKeyRecord {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct CreateApiKeyResponse {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) full_key: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct ListApiKeysResponse {
    pub(crate) keys: Vec<ApiKeyRecord>,
}
