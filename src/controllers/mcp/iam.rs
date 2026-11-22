//! MCP tools for IAM policies, groups, their attachments and the audit log.

use crate::controllers::mcp::verified;
use axum::http::request::Parts;
use rmcp::handler::server::{common::Extension, wrapper::Parameters};
use rmcp::{ErrorData, model::CallToolResult, tool, tool_router};

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::iam::{
    CreateGroup, CreatePolicy, CreateRole, GroupRecord, PolicyRecord, RoleRecord,
};
use crate::models::api_keys::ApiKeyScope;

fn iam_audit_requirement() -> rmcp::model::MetaObject {
    let mut meta = super::scope_requirement(ApiKeyScope::Read);
    meta.0.insert(
        "yorishiro/required_grants".into(),
        serde_json::json!(["audit"]),
    );
    meta
}

#[tool_router(vis = "pub(crate)", router = tool_router_iam)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM groups with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_groups(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_groups");
        match crate::models::iam_groups::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(rows.into_iter().map(GroupRecord::from).collect::<Vec<_>>()),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM policies with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_policies(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_policies");
        match crate::models::iam_policies::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(rows.into_iter().map(PolicyRecord::from).collect::<Vec<_>>()),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM group members with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_group_members(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListGroupAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_group_members");
        match crate::models::iam_group_memberships::Model::list_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(
                rows.into_iter()
                    .map(crate::dtos::iam::GroupMemberRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM user policies with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_user_policies(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListUserPolicies>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_user_policies");
        match crate::models::iam_user_policy_attachments::Model::list_as(
            &self.app_context().db,
            &auth,
            input.user_id,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(
                rows.into_iter()
                    .map(crate::dtos::iam::UserPolicyRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM group policies with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_group_policies(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListGroupAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_group_policies");
        match crate::models::iam_group_policy_attachments::Model::list_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(
                rows.into_iter()
                    .map(crate::dtos::iam::GroupPolicyRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Get a tenant IAM group (requires read scope and IAM permission)")]
    pub(crate) async fn get_iam_group(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "get_iam_group");
        match crate::models::iam_groups::Model::get_as(
            &self.app_context().db,
            &auth,
            input.group_id,
        )
        .await
        {
            Ok(row) => ok_json(GroupRecord::from(row)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Get a tenant IAM policy (requires read scope and IAM permission)")]
    pub(crate) async fn get_iam_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::PolicyId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "get_iam_policy");
        match crate::models::iam_policies::Model::get_as(
            &self.app_context().db,
            &auth,
            input.policy_id,
        )
        .await
        {
            Ok(row) => ok_json(PolicyRecord::from(row)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Update a tenant IAM policy and immediately re-evaluate its grants (requires read scope and IAM permission)")]
    pub(crate) async fn update_iam_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::UpdatePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "update_iam_policy");
        match crate::models::iam_policies::Model::update_as(
            &self.app_context().db,
            &auth,
            input.policy_id,
            input.name,
            input.document,
        )
        .await
        {
            Ok(policy) => ok_json(PolicyRecord::from(policy)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Delete a tenant IAM policy and its user and group attachments (requires read scope and IAM permission)")]
    pub(crate) async fn delete_iam_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::PolicyId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "delete_iam_policy");
        match crate::models::iam_policies::Model::delete_as(
            &self.app_context().db,
            &auth,
            input.policy_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"deleted":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Add group member in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn add_group_member(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "add_group_member");
        match crate::models::iam_group_memberships::Model::add_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            input.user_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Remove group member in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn remove_group_member(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "remove_group_member");
        match crate::models::iam_group_memberships::Model::remove_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            input.user_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Attach group policy in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn attach_group_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "attach_group_policy");
        match crate::models::iam_group_policy_attachments::Model::add_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            input.policy_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Detach group policy in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn detach_group_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "detach_group_policy");
        match crate::models::iam_group_policy_attachments::Model::remove_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            input.policy_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Attach a tenant IAM policy to a non-root tenant user (requires read scope and IAM permission)")]
    pub(crate) async fn attach_user_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::UserPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "attach_user_policy");
        match crate::models::iam_user_policy_attachments::Model::add_as(
            &self.app_context().db,
            &auth,
            input.user_id,
            input.policy_id,
        )
        .await
        {
            Ok(_) => ok_json(serde_json::json!({"attached":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Detach a tenant IAM policy from a tenant user (requires read scope and IAM permission)")]
    pub(crate) async fn detach_user_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::UserPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "detach_user_policy");
        match crate::models::iam_user_policy_attachments::Model::remove_as(
            &self.app_context().db,
            &auth,
            input.user_id,
            input.policy_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"detached":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Rename a tenant IAM group (requires read scope and IAM permission)")]
    pub(crate) async fn update_iam_group(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RenameGroup>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "update_iam_group");
        match crate::models::iam_groups::Model::rename_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            input.name,
        )
        .await
        {
            Ok(group) => ok_json(GroupRecord::from(group)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Delete a tenant IAM group and its memberships and policy attachments (requires read scope and IAM permission)")]
    pub(crate) async fn delete_iam_group(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "delete_iam_group");
        match crate::models::iam_groups::Model::delete_as(
            &self.app_context().db,
            &auth,
            input.group_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"deleted":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = iam_audit_requirement(), description = "List tenant IAM audit events (requires read scope, audit grant and IAM permission)")]
    pub(crate) async fn list_iam_audit(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListAudit>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_audit");
        match crate::models::iam_audit_events::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(events) => ok_json(
                events
                    .into_iter()
                    .map(crate::dtos::iam::AuditRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Create a tenant IAM group (requires read scope and IAM permission)")]
    pub(crate) async fn create_iam_group(
        &self,
        Parameters(args): Parameters<CreateGroup>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "create_iam_group");
        match crate::models::iam_groups::Model::create_as(&self.app_context().db, &auth, args.name)
            .await
        {
            Ok(group) => ok_json(GroupRecord::from(group)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Create a tenant IAM policy (requires read scope and IAM permission)")]
    pub(crate) async fn create_iam_policy(
        &self,
        Parameters(args): Parameters<CreatePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "create_iam_policy");
        match crate::models::iam_policies::Model::create_as(
            &self.app_context().db,
            &auth,
            args.name,
            args.document,
        )
        .await
        {
            Ok(policy) => ok_json(PolicyRecord::from(policy)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM roles with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_roles(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_roles");
        match crate::models::iam_roles::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(rows.into_iter().map(RoleRecord::from).collect::<Vec<_>>()),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM role members with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_role_members(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListRoleAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_role_members");
        match crate::models::iam_user_role_attachments::Model::list_as(
            &self.app_context().db,
            &auth,
            input.role_id,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(
                rows.into_iter()
                    .map(crate::dtos::iam::RoleMemberRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM role policies with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_role_policies(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListRoleAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "list_iam_role_policies");
        match crate::models::iam_role_policy_attachments::Model::list_as(
            &self.app_context().db,
            &auth,
            input.role_id,
            crate::models::pagination::ListParams::new(input.limit, input.offset),
        )
        .await
        {
            Ok(rows) => ok_json(
                rows.into_iter()
                    .map(crate::dtos::iam::RolePolicyRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Get a tenant IAM role (requires read scope and IAM permission)")]
    pub(crate) async fn get_iam_role(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RoleId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "get_iam_role");
        match crate::models::iam_roles::Model::get_as(&self.app_context().db, &auth, input.role_id)
            .await
        {
            Ok(row) => ok_json(RoleRecord::from(row)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Add role member in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn add_role_member(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RoleMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "add_role_member");
        match crate::models::iam_user_role_attachments::Model::add_as(
            &self.app_context().db,
            &auth,
            input.role_id,
            input.user_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Remove role member in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn remove_role_member(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RoleMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "remove_role_member");
        match crate::models::iam_user_role_attachments::Model::remove_as(
            &self.app_context().db,
            &auth,
            input.role_id,
            input.user_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Attach role policy in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn attach_role_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RolePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "attach_role_policy");
        match crate::models::iam_role_policy_attachments::Model::add_as(
            &self.app_context().db,
            &auth,
            input.role_id,
            input.policy_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Detach role policy in the current tenant (requires read scope and IAM permission)")]
    pub(crate) async fn detach_role_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RolePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "detach_role_policy");
        match crate::models::iam_role_policy_attachments::Model::remove_as(
            &self.app_context().db,
            &auth,
            input.role_id,
            input.policy_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"changed":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Rename a tenant IAM role (requires read scope and IAM permission)")]
    pub(crate) async fn update_iam_role(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RenameRole>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "update_iam_role");
        match crate::models::iam_roles::Model::rename_as(
            &self.app_context().db,
            &auth,
            input.role_id,
            input.name,
        )
        .await
        {
            Ok(role) => ok_json(RoleRecord::from(role)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Delete a tenant IAM role and its memberships and policy attachments (requires read scope and IAM permission)")]
    pub(crate) async fn delete_iam_role(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RoleId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "delete_iam_role");
        match crate::models::iam_roles::Model::delete_as(
            &self.app_context().db,
            &auth,
            input.role_id,
        )
        .await
        {
            Ok(()) => ok_json(serde_json::json!({"deleted":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Create a tenant IAM role (requires read scope and IAM permission)")]
    pub(crate) async fn create_iam_role(
        &self,
        Parameters(args): Parameters<CreateRole>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts, "create_iam_role");
        match crate::models::iam_roles::Model::create_as(&self.app_context().db, &auth, args.name)
            .await
        {
            Ok(role) => ok_json(RoleRecord::from(role)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
