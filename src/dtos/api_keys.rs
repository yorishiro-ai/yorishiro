use crate::data::time::Timezone;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateApiKeyRequest {
    pub(crate) name: String,
    pub(crate) expires_at: Option<DateTime<chrono::FixedOffset>>,
    #[serde(default)]
    pub(crate) audit: bool,
    #[expect(clippy::doc_markdown, reason = "MCP exposes this description verbatim")]
    /// Issue a tenant-scoped key without a workspace binding.
    /// Requires CredentialCreate IAM permission and a tenant-scoped issuing credential.
    #[serde(default)]
    pub(crate) tenant_scoped: bool,
}

impl CreateApiKeyRequest {
    pub(crate) fn validate_workspace_selection(
        &self,
        selected: bool,
    ) -> Result<(), crate::YorishiroError> {
        if self.tenant_scoped && selected {
            return Err(crate::YorishiroError::ValidationFailed {
                message: "tenant_scoped cannot be combined with a workspace selector".into(),
                details: vec![],
                hint: "omit the workspace selector for an unbound key".into(),
            });
        }
        Ok(())
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ApiKeyRecord {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Timezone>,
    pub(crate) last_used_at: Option<DateTime<Timezone>>,
    #[serde(flatten)]
    pub(crate) grant: crate::models::api_keys::KeyGrant,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct CreateApiKeyResponse {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) full_key: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Timezone>,
    #[serde(flatten)]
    pub(crate) grant: crate::models::api_keys::KeyGrant,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ListApiKeysResponse {
    pub(crate) keys: Vec<ApiKeyRecord>,
}
