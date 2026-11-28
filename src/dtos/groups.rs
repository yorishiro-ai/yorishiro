//! Request and response types for groups.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateGroup {
    /// Unique within the tenant, ignoring case: 1 to 128 characters from letters, digits and `+ = , . @ _ -`.
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListGroupAssociations {
    pub group_id: Uuid,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

crate::dtos::common::list_params_from!(ListGroupAssociations);

#[derive(Serialize, JsonSchema)]
pub(crate) struct GroupRecord {
    pub group_id: Uuid,
    pub tenant_id: Uuid,
    /// `yrn:iam:<tenant-id>:group/<name>`.
    pub yrn: String,
    pub name: String,
}

impl From<crate::models::iam_groups::Model> for GroupRecord {
    fn from(group: crate::models::iam_groups::Model) -> Self {
        Self {
            yrn: group.yrn(),
            group_id: group.id,
            tenant_id: group.tenant_id,
            name: group.name,
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct GroupMemberRecord {
    pub user_id: Uuid,
    pub yrn: String,
    pub user_name: Option<String>,
    pub display_name: Option<String>,
}

/// A group and the users that belong to it.
#[derive(Serialize, JsonSchema)]
pub(crate) struct GroupDetail {
    #[serde(flatten)]
    pub group: GroupRecord,
    pub members: Vec<GroupMemberRecord>,
}

impl From<crate::models::iam_groups::GroupView> for GroupDetail {
    fn from(view: crate::models::iam_groups::GroupView) -> Self {
        let tenant_id = view.group.tenant_id;
        Self {
            members: view
                .members
                .into_iter()
                .map(|user| GroupMemberRecord {
                    user_id: user.id,
                    yrn: user.yrn(tenant_id),
                    user_name: user.user_name,
                    display_name: user.display_name,
                })
                .collect(),
            group: view.group.into(),
        }
    }
}
