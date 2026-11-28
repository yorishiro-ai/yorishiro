use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::YorishiroMcpServer;
use crate::dtos::groups::{
    CreateGroup, GroupDetail, GroupId, GroupMember, GroupPolicy, GroupRecord,
    ListGroupAssociations, RenameGroup,
};
use crate::dtos::iam::ListIam;
use crate::dtos::policies::AttachedPolicyRecord;
use crate::models::pagination::ListParams;

#[tool_router(vis = "pub(crate)", router = tool_router_groups)]
impl YorishiroMcpServer {
    #[tool(
        description = "List the groups of the tenant with bounded pagination (requires iam:ListGroups)"
    )]
    pub(crate) async fn list_groups(
        &self,
        Parameters(input): Parameters<ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_groups::Model::list_as(db, &auth, ListParams::from(&input))
                .await
                .map(|rows| rows.into_iter().map(GroupRecord::from).collect::<Vec<_>>()),
        )
    }

    #[tool(
        description = "Create a group; names are unique per tenant ignoring case (requires iam:CreateGroup)"
    )]
    pub(crate) async fn create_group(
        &self,
        Parameters(input): Parameters<CreateGroup>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_groups::Model::create_as(db, &auth, input.name)
                .await
                .map(GroupRecord::from),
        )
    }

    #[tool(description = "Get a group with the users that belong to it (requires iam:GetGroup)")]
    pub(crate) async fn get_group(
        &self,
        Parameters(input): Parameters<GroupId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_groups::Model::get_as(db, &auth, input.group_id)
                .await
                .map(GroupDetail::from),
        )
    }

    #[tool(description = "Rename a group (requires iam:UpdateGroup)")]
    pub(crate) async fn update_group(
        &self,
        Parameters(input): Parameters<RenameGroup>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_groups::Model::rename_as(db, &auth, input.group_id, input.name)
                .await
                .map(GroupRecord::from),
        )
    }

    #[tool(
        description = "Delete a group; refused with a conflict while it has members or attached policies (requires iam:DeleteGroup)"
    )]
    pub(crate) async fn delete_group(
        &self,
        Parameters(input): Parameters<GroupId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_groups::Model::delete_as(db, &auth, input.group_id)
                .await
                .map(|()| serde_json::json!({"deleted": true})),
        )
    }

    #[tool(
        description = "Add a user to a group; joining a group that carries AdministratorAccess requires holding it or being the tenant root (requires iam:AddUserToGroup)"
    )]
    pub(crate) async fn add_user_to_group(
        &self,
        Parameters(input): Parameters<GroupMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_group_memberships::Model::add_as(
                db,
                &auth,
                input.group_id,
                input.user_id,
            )
            .await
            .map(|()| serde_json::json!({"added": true})),
        )
    }

    #[tool(description = "Remove a user from a group (requires iam:RemoveUserFromGroup)")]
    pub(crate) async fn remove_user_from_group(
        &self,
        Parameters(input): Parameters<GroupMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_group_memberships::Model::remove_as(
                db,
                &auth,
                input.group_id,
                input.user_id,
            )
            .await
            .map(|()| serde_json::json!({"removed": true})),
        )
    }

    #[tool(
        description = "Attach a policy to a group; attaching AdministratorAccess requires holding it or being the tenant root (requires iam:AttachGroupPolicy)"
    )]
    pub(crate) async fn attach_group_policy(
        &self,
        Parameters(input): Parameters<GroupPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_group_policy_attachments::Model::add_as(
                db,
                &auth,
                input.group_id,
                input.policy_id,
            )
            .await
            .map(|()| serde_json::json!({"attached": true})),
        )
    }

    #[tool(description = "Detach a policy from a group (requires iam:DetachGroupPolicy)")]
    pub(crate) async fn detach_group_policy(
        &self,
        Parameters(input): Parameters<GroupPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_group_policy_attachments::Model::remove_as(
                db,
                &auth,
                input.group_id,
                input.policy_id,
            )
            .await
            .map(|()| serde_json::json!({"detached": true})),
        )
    }

    #[tool(
        description = "List the policies attached to a group (requires iam:ListAttachedGroupPolicies)"
    )]
    pub(crate) async fn list_attached_group_policies(
        &self,
        Parameters(input): Parameters<ListGroupAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_group_policy_attachments::Model::list_as(
                db,
                &auth,
                input.group_id,
                ListParams::from(&input),
            )
            .await
            .map(|policies| {
                policies
                    .into_iter()
                    .map(AttachedPolicyRecord::from)
                    .collect::<Vec<_>>()
            }),
        )
    }
}
