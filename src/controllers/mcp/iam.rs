//! MCP tools for IAM: policies, groups, roles, their attachments, role assumption and the audit log.

use axum::http::request::Parts;
use rmcp::handler::server::{common::Extension, wrapper::Parameters};
use rmcp::{ErrorData, model::CallToolResult, tool, tool_router};

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::iam::{CreateGroup, CreatePolicy, GroupRecord, PolicyRecord};
use crate::models::api_keys::ApiKeyScope;

#[tool_router(vis = "pub(crate)", router = tool_router_iam)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM groups with bounded pagination (requires read scope and IAM permission)")]
    pub(crate) async fn list_iam_groups(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_groups")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_policies")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_group_members")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_user_policies")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_group_policies")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("get_iam_group")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("get_iam_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("update_iam_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("delete_iam_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("add_group_member")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("remove_group_member")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("attach_group_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("detach_group_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("attach_user_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("detach_user_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("update_iam_group")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("delete_iam_group")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_audit")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_iam_group")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_iam_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
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
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Create a tenant IAM role with a trust policy naming who may assume it (requires read scope and role_create IAM permission)")]
    pub(crate) async fn create_iam_role(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::CreateRole>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_iam_role")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_roles::Model::create_as(
            &self.app_context().db,
            &auth,
            crate::models::iam_roles::CreateRole {
                name: args.name,
                trust_policy: args.trust_policy,
                max_session_seconds: args.max_session_seconds,
            },
        )
        .await
        {
            Ok(value) => ok_json(crate::dtos::iam::RoleRecord::from(value)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List tenant IAM roles with bounded pagination (requires read scope and role_list IAM permission)")]
    pub(crate) async fn list_iam_roles(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_roles")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_roles::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::new(args.limit, args.offset),
        )
        .await
        {
            Ok(value) => ok_json(
                value
                    .into_iter()
                    .map(crate::dtos::iam::RoleRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Get a tenant IAM role (requires read scope and role_get IAM permission)")]
    pub(crate) async fn get_iam_role(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::RoleId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("get_iam_role")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_roles::Model::get_as(&self.app_context().db, &auth, args.role_id)
            .await
        {
            Ok(value) => ok_json(crate::dtos::iam::RoleRecord::from(value)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Update a tenant IAM role's name, trust policy or maximum session length (requires read scope and role_update IAM permission)")]
    pub(crate) async fn update_iam_role(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::UpdateRoleArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("update_iam_role")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_roles::Model::update_as(
            &self.app_context().db,
            &auth,
            args.role_id,
            crate::models::iam_roles::UpdateRole {
                name: args.name,
                trust_policy: args.trust_policy,
                max_session_seconds: args.max_session_seconds,
            },
        )
        .await
        {
            Ok(value) => ok_json(crate::dtos::iam::RoleRecord::from(value)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Delete a tenant IAM role and revoke every session assumed from it (requires read scope and role_delete IAM permission)")]
    pub(crate) async fn delete_iam_role(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::RoleId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("delete_iam_role")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_roles::Model::delete_as(
            &self.app_context().db,
            &auth,
            args.role_id,
        )
        .await
        {
            Ok(_) => ok_json(serde_json::json!({"deleted": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "List the managed policies attached to a role (requires read scope and role_policy_list IAM permission)")]
    pub(crate) async fn list_iam_role_policies(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::ListRolePolicies>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_iam_role_policies")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_role_policy_attachments::Model::list_as(
            &self.app_context().db,
            &auth,
            args.role_id,
            crate::models::pagination::ListParams::new(args.limit, args.offset),
        )
        .await
        {
            Ok(value) => ok_json(
                value
                    .into_iter()
                    .map(crate::dtos::iam::RolePolicyRecord::from)
                    .collect::<Vec<_>>(),
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Attach a managed policy to a role, at most 20 per role (requires read scope and role_policy_attach IAM permission)")]
    pub(crate) async fn attach_role_policy(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::RolePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("attach_role_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_role_policy_attachments::Model::add_as(
            &self.app_context().db,
            &auth,
            args.role_id,
            args.policy_id,
        )
        .await
        {
            Ok(_) => ok_json(serde_json::json!({"attached": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Detach a managed policy from a role (requires read scope and role_policy_detach IAM permission)")]
    pub(crate) async fn detach_role_policy(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::RolePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("detach_role_policy")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_role_policy_attachments::Model::remove_as(
            &self.app_context().db,
            &auth,
            args.role_id,
            args.policy_id,
        )
        .await
        {
            Ok(_) => ok_json(serde_json::json!({"detached": true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Assume an IAM role for a temporary credential shown once; the role trust policy and a role_assume permission must both allow it, and the session is evaluated by the role's policies alone (requires read scope; the tenant root and role sessions cannot assume roles)")]
    pub(crate) async fn assume_role(
        &self,
        Parameters(args): Parameters<crate::dtos::iam::AssumeRoleArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("assume_role")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match crate::models::iam_roles::Model::assume_as(
            &self.app_context().db,
            &auth,
            args.role_id,
            args.scope,
            args.duration_seconds,
        )
        .await
        {
            Ok(value) => ok_json(value),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}

fn iam_audit_requirement() -> rmcp::model::MetaObject {
    let mut meta = super::scope_requirement(ApiKeyScope::Read);
    meta.0.insert(
        "yorishiro/required_grants".into(),
        serde_json::json!(["audit"]),
    );
    meta
}
