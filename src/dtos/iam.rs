//! Request and response types for the IAM endpoints, including conversions from the IAM models.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdatePolicy {
    pub policy_id: Uuid,
    pub name: String,
    pub document: serde_json::Value,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyId {
    pub policy_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserPolicy {
    pub user_id: Uuid,
    pub policy_id: Uuid,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListAudit {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct AuditRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub api_key_id: Uuid,
    pub action: String,
    pub detail: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

impl From<crate::models::iam_audit_events::Model> for AuditRecord {
    fn from(event: crate::models::iam_audit_events::Model) -> Self {
        Self {
            id: event.id,
            tenant_id: event.tenant_id,
            user_id: event.user_id,
            api_key_id: event.api_key_id,
            action: event.action,
            detail: event.detail,
            created_at: event.created_at,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateGroup {
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenameGroup {
    pub group_id: Uuid,
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupId {
    pub group_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatePolicy {
    pub name: String,
    pub document: serde_json::Value,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct GroupRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
}

impl From<crate::models::iam_groups::Model> for GroupRecord {
    fn from(group: crate::models::iam_groups::Model) -> Self {
        Self {
            id: group.id,
            tenant_id: group.tenant_id,
            name: group.name,
        }
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct PolicyRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub document: serde_json::Value,
}

impl From<crate::models::iam_policies::Model> for PolicyRecord {
    fn from(policy: crate::models::iam_policies::Model) -> Self {
        Self {
            id: policy.id,
            tenant_id: policy.tenant_id,
            name: policy.name,
            document: policy.document,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupMember {
    pub group_id: Uuid,
    pub user_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupPolicy {
    pub group_id: Uuid,
    pub policy_id: Uuid,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListIam {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListGroupAssociations {
    pub group_id: Uuid,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListUserPolicies {
    pub user_id: Uuid,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct GroupMemberRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub group_id: Uuid,
    pub user_id: Uuid,
}
impl From<crate::models::iam_group_memberships::Model> for GroupMemberRecord {
    fn from(row: crate::models::iam_group_memberships::Model) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            group_id: row.group_id,
            user_id: row.user_id,
        }
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct UserPolicyRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub policy_id: Uuid,
}
impl From<crate::models::iam_user_policy_attachments::Model> for UserPolicyRecord {
    fn from(row: crate::models::iam_user_policy_attachments::Model) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            user_id: row.user_id,
            policy_id: row.policy_id,
        }
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct GroupPolicyRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub group_id: Uuid,
    pub policy_id: Uuid,
}
impl From<crate::models::iam_group_policy_attachments::Model> for GroupPolicyRecord {
    fn from(row: crate::models::iam_group_policy_attachments::Model) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            group_id: row.group_id,
            policy_id: row.policy_id,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateRole {
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenameRole {
    pub role_id: Uuid,
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoleId {
    pub role_id: Uuid,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct RoleRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
}

impl From<crate::models::iam_roles::Model> for RoleRecord {
    fn from(row: crate::models::iam_roles::Model) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            name: row.name,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoleMember {
    pub role_id: Uuid,
    pub user_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RolePolicy {
    pub role_id: Uuid,
    pub policy_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListRoleAssociations {
    pub role_id: Uuid,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct RoleMemberRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub role_id: Uuid,
    pub user_id: Uuid,
}
impl From<crate::models::iam_user_role_attachments::Model> for RoleMemberRecord {
    fn from(row: crate::models::iam_user_role_attachments::Model) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            role_id: row.role_id,
            user_id: row.user_id,
        }
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct RolePolicyRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub role_id: Uuid,
    pub policy_id: Uuid,
}
impl From<crate::models::iam_role_policy_attachments::Model> for RolePolicyRecord {
    fn from(row: crate::models::iam_role_policy_attachments::Model) -> Self {
        Self {
            id: row.id,
            tenant_id: row.tenant_id,
            role_id: row.role_id,
            policy_id: row.policy_id,
        }
    }
}

crate::dtos::common::list_params_from!(
    ListAudit,
    ListIam,
    ListGroupAssociations,
    ListUserPolicies,
    ListRoleAssociations,
);
