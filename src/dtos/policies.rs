//! Request and response types for policies, their versions and attachments.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatePolicy {
    /// Unique within the tenant: 1 to 128 characters from letters, digits and `+ = , . @ _ -`.
    /// A name cannot change, and the names of the managed policies are reserved.
    pub name: String,
    /// The first version of the policy, which becomes its default version.
    /// A document is `{"Version": "2012-10-17", "Statement": [...]}`.
    /// A statement holds an optional `Sid`, an `Effect` of `Allow` or `Deny`, exactly one of `Action` and `NotAction`, and a `Resource`.
    /// `Action`, `NotAction` and `Resource` take one string or a list of strings.
    /// An action is `*`, `<service>:*` or `<service>:<Name>` such as `iam:*`, `iam:CreateGroup` or `entity:Get*`, where the name may use the wildcards `*` and `?`.
    /// A resource is `*`, the tenant YRN `yrn:tenant:<tenant-id>`, or a workspace YRN `yrn:workspace:<tenant-id>:<workspace-id>` where the id may be `*`.
    /// An explicit `Deny` beats every `Allow`.
    #[schemars(with = "crate::models::iam::policy::RawDocument")]
    pub document: serde_json::Value,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatePolicyVersionRequest {
    /// The new version's document, in the same language as the policy's first version.
    #[schemars(with = "crate::models::iam::policy::RawDocument")]
    pub document: serde_json::Value,
    /// Makes the new version the default version, which evaluation uses.
    #[serde(default)]
    pub set_as_default: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatePolicyVersionArgs {
    pub policy_id: Uuid,
    /// The new version's document: `{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": "iam:List*", "Resource": "*"}]}`.
    /// A policy holds at most 5 versions.
    #[schemars(with = "crate::models::iam::policy::RawDocument")]
    pub document: serde_json::Value,
    /// Makes the new version the default version, which evaluation uses.
    #[serde(default)]
    pub set_as_default: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyId {
    pub policy_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyVersionId {
    pub policy_id: Uuid,
    pub version_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct PolicyVersionIdPath<T = Uuid> {
    pub policy_id: Uuid,
    pub version_id: T,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserPolicy {
    pub user_id: Uuid,
    pub policy_id: Uuid,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PolicyRecord {
    pub policy_id: Uuid,
    pub tenant_id: Uuid,
    /// `yrn:iam:<tenant-id>:policy/<name>`; a managed policy uses `yorishiro` in place of the tenant id.
    pub yrn: String,
    pub name: String,
    /// A managed policy is defined by the product and cannot be versioned or deleted.
    pub managed: bool,
    pub default_version_id: Uuid,
    /// The document of the default version.
    pub document: serde_json::Value,
}

impl From<crate::models::iam_policies::PolicyView> for PolicyRecord {
    fn from(policy: crate::models::iam_policies::PolicyView) -> Self {
        Self {
            policy_id: policy.id,
            tenant_id: policy.tenant_id,
            yrn: policy.yrn,
            name: policy.name,
            managed: policy.managed,
            default_version_id: policy.default_version_id,
            document: policy.document,
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct PolicyVersionRecord {
    pub id: Uuid,
    pub policy_id: Uuid,
    pub version_number: i32,
    pub is_default: bool,
    pub document: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

impl From<crate::models::iam_policy_versions::PolicyVersion> for PolicyVersionRecord {
    fn from(version: crate::models::iam_policy_versions::PolicyVersion) -> Self {
        Self {
            id: version.id,
            policy_id: version.policy_id,
            version_number: version.version_number,
            is_default: version.is_default,
            document: version.document,
            created_at: version.created_at,
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct AttachedPolicyRecord {
    pub policy_id: Uuid,
    pub name: String,
    pub yrn: String,
}

impl From<crate::models::iam_policies::AttachedPolicy> for AttachedPolicyRecord {
    fn from(policy: crate::models::iam_policies::AttachedPolicy) -> Self {
        Self {
            policy_id: policy.policy_id,
            name: policy.name,
            yrn: policy.yrn,
        }
    }
}
