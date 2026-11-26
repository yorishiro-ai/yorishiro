use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::YorishiroMcpServer;
use crate::dtos::groups::GroupRecord;
use crate::dtos::iam::ListIam;
use crate::dtos::policies::{AttachedPolicyRecord, UserPolicy};
use crate::dtos::users::{
    CreateUserRequest, ListUserAssociations, UpdateLoginProfileArgs, UpdateUserArgs, UserId,
    UserRecord,
};
use crate::models::pagination::ListParams;

#[tool_router(vis = "pub(crate)", router = tool_router_users)]
impl YorishiroMcpServer {
    #[tool(
        description = "List the users of the tenant, excluding the tenant root, with bounded pagination (requires iam:ListUsers)"
    )]
    pub(crate) async fn list_users(
        &self,
        Parameters(input): Parameters<ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam::users::list_as(db, &auth, ListParams::from(&input))
                .await
                .map(|users| {
                    users
                        .into_iter()
                        .map(|user| UserRecord::render(self.app_context(), auth.tenant_id, user))
                        .collect::<Vec<_>>()
                }),
        )
    }

    #[tool(
        description = "Create a user with a tenant-local user name and password; the user starts with no permissions until policies or groups are attached (requires iam:CreateUser)"
    )]
    pub(crate) async fn create_user(
        &self,
        Parameters(input): Parameters<CreateUserRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam::users::create_as(
                db,
                &auth,
                &input.user_name,
                &input.password,
                input.display_name.as_deref(),
            )
            .await
            .map(|user| UserRecord::render(self.app_context(), auth.tenant_id, user)),
        )
    }

    #[tool(description = "Get a user by id, including the caller itself (requires iam:GetUser)")]
    pub(crate) async fn get_user(
        &self,
        Parameters(input): Parameters<UserId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam::users::get_as(db, &auth, input.user_id)
                .await
                .map(|user| UserRecord::render(self.app_context(), auth.tenant_id, user)),
        )
    }

    #[tool(
        description = "Update or clear the display name of a user; the caller edits itself through its own user id (requires iam:UpdateUser)"
    )]
    pub(crate) async fn update_user(
        &self,
        Parameters(input): Parameters<UpdateUserArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam::users::update_as(db, &auth, input.user_id, input.display_name)
                .await
                .map(|user| UserRecord::render(self.app_context(), auth.tenant_id, user)),
        )
    }

    #[tool(
        description = "Delete a user; refused with a conflict while the user still has group memberships, attached policies or access keys, and for the tenant root (requires iam:DeleteUser)"
    )]
    pub(crate) async fn delete_user(
        &self,
        Parameters(input): Parameters<UserId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam::users::delete_as(db, &auth, input.user_id)
                .await
                .map(|()| serde_json::json!({"deleted": true})),
        )
    }

    #[tool(
        description = "Set the password of a user and revoke every access key issued to the user; tenant root recovery is CLI-only (requires iam:UpdateLoginProfile)"
    )]
    pub(crate) async fn update_login_profile(
        &self,
        Parameters(input): Parameters<UpdateLoginProfileArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam::users::update_login_profile_as(
                db,
                &auth,
                input.user_id,
                &input.password,
            )
            .await
            .map(|()| serde_json::json!({"updated": true})),
        )
    }

    #[tool(description = "List the groups a user belongs to (requires iam:ListGroupsForUser)")]
    pub(crate) async fn list_groups_for_user(
        &self,
        Parameters(input): Parameters<ListUserAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_group_memberships::Model::groups_for_user_as(
                db,
                &auth,
                input.user_id,
                ListParams::from(&input),
            )
            .await
            .map(|groups| {
                groups
                    .into_iter()
                    .map(GroupRecord::from)
                    .collect::<Vec<_>>()
            }),
        )
    }

    #[tool(
        description = "List the policies attached directly to a user (requires iam:ListAttachedUserPolicies)"
    )]
    pub(crate) async fn list_attached_user_policies(
        &self,
        Parameters(input): Parameters<ListUserAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_user_policy_attachments::Model::list_as(
                db,
                &auth,
                input.user_id,
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

    #[tool(
        description = "Attach a policy to a user; attaching AdministratorAccess requires holding it or being the tenant root (requires iam:AttachUserPolicy)"
    )]
    pub(crate) async fn attach_user_policy(
        &self,
        Parameters(input): Parameters<UserPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_user_policy_attachments::Model::add_as(
                db,
                &auth,
                input.user_id,
                input.policy_id,
            )
            .await
            .map(|()| serde_json::json!({"attached": true})),
        )
    }

    #[tool(description = "Detach a policy from a user (requires iam:DetachUserPolicy)")]
    pub(crate) async fn detach_user_policy(
        &self,
        Parameters(input): Parameters<UserPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_user_policy_attachments::Model::remove_as(
                db,
                &auth,
                input.user_id,
                input.policy_id,
            )
            .await
            .map(|()| serde_json::json!({"detached": true})),
        )
    }
}
