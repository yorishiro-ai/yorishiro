use crate::data::time::Timezone;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateAccessKeyRequest {
    pub(crate) name: String,
    pub(crate) expires_at: Option<DateTime<chrono::FixedOffset>>,
    /// Issue a tenant-scoped key without a workspace binding.
    /// Requires the `iam:CreateAccessKey` action and a tenant-scoped issuing credential.
    #[serde(default)]
    pub(crate) tenant_scoped: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct AccessKeyIdPath {
    pub(crate) access_key_id: Uuid,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateAccessKeyRequest {
    pub(crate) name: Option<String>,
    /// The new expiry.
    /// Takes effect only with `replace_expires_at`; leave `expires_at` out to clear the expiry.
    pub(crate) expires_at: Option<DateTime<chrono::FixedOffset>>,
    #[serde(default)]
    pub(crate) replace_expires_at: bool,
    /// `false` disables the key so it no longer authenticates; `true` enables it again.
    pub(crate) enabled: Option<bool>,
}

impl UpdateAccessKeyRequest {
    pub(crate) fn into_change(self) -> crate::models::access_keys::AccessKeyChange {
        crate::models::access_keys::AccessKeyChange {
            name: self.name,
            expires_at: self.expires_at,
            replace_expires_at: self.replace_expires_at,
            enabled: self.enabled,
        }
    }
}

impl CreateAccessKeyRequest {
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
pub(crate) struct AccessKeyRecord {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Timezone>,
    pub(crate) last_used_at: Option<DateTime<Timezone>>,
    #[serde(flatten)]
    pub(crate) grant: crate::models::access_keys::KeyGrant,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct CreateAccessKeyResponse {
    pub(crate) id: Uuid,
    pub(crate) prefix: String,
    pub(crate) full_key: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Timezone>,
    #[serde(flatten)]
    pub(crate) grant: crate::models::access_keys::KeyGrant,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ListAccessKeysResponse {
    pub(crate) keys: Vec<AccessKeyRecord>,
}
