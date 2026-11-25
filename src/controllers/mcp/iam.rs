//! MCP tools for IAM policies, groups, their attachments and the audit log.

use crate::controllers::mcp::verified;
use axum::http::request::Parts;
use rmcp::handler::server::{common::Extension, wrapper::Parameters};
use rmcp::{ErrorData, model::CallToolResult, tool, tool_router};

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::dtos::iam::{CreateGroup, CreatePolicy, GroupRecord, PolicyRecord};

fn iam_audit_requirement() -> rmcp::model::MetaObject {
    let mut meta = rmcp::model::MetaObject::new();
    meta.0.insert(
        "yorishiro/required_grants".into(),
        serde_json::json!(["audit"]),
    );
    meta
}

#[tool_router(vis = "pub(crate)", router = tool_router_iam)]
impl YorishiroMcpServer {
    #[tool(
        description = "List tenant IAM groups with bounded pagination (requires IAM permission)"
    )]
    pub(crate) async fn list_iam_groups(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam_groups::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::from(&input),
        )
        .await
        {
            Ok(rows) => ok_json(rows.into_iter().map(GroupRecord::from).collect::<Vec<_>>()),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "List tenant IAM policies with bounded pagination (requires IAM permission)"
    )]
    pub(crate) async fn list_iam_policies(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam_policies::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::from(&input),
        )
        .await
        {
            Ok(rows) => ok_json(rows.into_iter().map(PolicyRecord::from).collect::<Vec<_>>()),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "List tenant IAM group members with bounded pagination (requires IAM permission)"
    )]
    pub(crate) async fn list_iam_group_members(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListGroupAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam_group_memberships::Model::list_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            crate::models::pagination::ListParams::from(&input),
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

    #[tool(
        description = "List tenant IAM user policies with bounded pagination (requires IAM permission)"
    )]
    pub(crate) async fn list_iam_user_policies(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListUserPolicies>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam_user_policy_attachments::Model::list_as(
            &self.app_context().db,
            &auth,
            input.user_id,
            crate::models::pagination::ListParams::from(&input),
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

    #[tool(
        description = "List tenant IAM group policies with bounded pagination (requires IAM permission)"
    )]
    pub(crate) async fn list_iam_group_policies(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListGroupAssociations>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam_group_policy_attachments::Model::list_as(
            &self.app_context().db,
            &auth,
            input.group_id,
            crate::models::pagination::ListParams::from(&input),
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

    #[tool(description = "Get a tenant IAM group (requires IAM permission)")]
    pub(crate) async fn get_iam_group(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(description = "Get a tenant IAM policy (requires IAM permission)")]
    pub(crate) async fn get_iam_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::PolicyId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(
        description = "Update a tenant IAM policy and immediately re-evaluate its grants (requires IAM permission)"
    )]
    pub(crate) async fn update_iam_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::UpdatePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(
        description = "Delete a tenant IAM policy and its user and group attachments (requires IAM permission)"
    )]
    pub(crate) async fn delete_iam_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::PolicyId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(description = "Add group member in the current tenant (requires IAM permission)")]
    pub(crate) async fn add_group_member(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(description = "Remove group member in the current tenant (requires IAM permission)")]
    pub(crate) async fn remove_group_member(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupMember>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(description = "Attach group policy in the current tenant (requires IAM permission)")]
    pub(crate) async fn attach_group_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(description = "Detach group policy in the current tenant (requires IAM permission)")]
    pub(crate) async fn detach_group_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(
        description = "Attach a tenant IAM policy to a non-root tenant user (requires IAM permission)"
    )]
    pub(crate) async fn attach_user_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::UserPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(description = "Detach a tenant IAM policy from a tenant user (requires IAM permission)")]
    pub(crate) async fn detach_user_policy(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::UserPolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(description = "Rename a tenant IAM group (requires IAM permission)")]
    pub(crate) async fn update_iam_group(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::RenameGroup>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(
        description = "Delete a tenant IAM group and its memberships and policy attachments (requires IAM permission)"
    )]
    pub(crate) async fn delete_iam_group(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::GroupId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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

    #[tool(meta = iam_audit_requirement(), description = "List tenant IAM audit events (requires audit grant and IAM permission)")]
    pub(crate) async fn list_iam_audit(
        &self,
        Parameters(input): Parameters<crate::dtos::iam::ListAudit>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam_audit_events::Model::list_as(
            &self.app_context().db,
            &auth,
            crate::models::pagination::ListParams::from(&input),
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

    #[tool(description = "Create a tenant IAM group (requires IAM permission)")]
    pub(crate) async fn create_iam_group(
        &self,
        Parameters(args): Parameters<CreateGroup>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match crate::models::iam_groups::Model::create_as(&self.app_context().db, &auth, args.name)
            .await
        {
            Ok(group) => ok_json(GroupRecord::from(group)),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(description = "Create a tenant IAM policy (requires IAM permission)")]
    pub(crate) async fn create_iam_policy(
        &self,
        Parameters(args): Parameters<CreatePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
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
}
