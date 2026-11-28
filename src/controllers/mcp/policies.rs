use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::YorishiroMcpServer;
use crate::dtos::iam::ListIam;
use crate::dtos::policies::{
    CreatePolicy, CreatePolicyVersionArgs, PolicyId, PolicyRecord, PolicyVersionId,
    PolicyVersionRecord,
};
use crate::models::pagination::ListParams;

#[tool_router(vis = "pub(crate)", router = tool_router_policies)]
impl YorishiroMcpServer {
    #[tool(
        description = "List the policies of the tenant, including the managed policies, with bounded pagination (requires iam:ListPolicies)"
    )]
    pub(crate) async fn list_policies(
        &self,
        Parameters(input): Parameters<ListIam>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policies::Model::list_as(db, &auth, ListParams::from(&input))
                .await
                .map(|rows| rows.into_iter().map(PolicyRecord::from).collect::<Vec<_>>()),
        )
    }

    #[tool(
        description = "Create a policy whose first version is the default; names are immutable and the document changes only through versions. A document holds Version 2012-10-17 and a Statement list; a statement has an optional Sid, an Effect of Allow or Deny, exactly one of Action or NotAction such as iam:* or iam:Get*, and a Resource of * or a tenant or workspace YRN; an explicit Deny beats every Allow (requires iam:CreatePolicy)"
    )]
    pub(crate) async fn create_policy(
        &self,
        Parameters(input): Parameters<CreatePolicy>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policies::Model::create_as(db, &auth, input.name, input.document)
                .await
                .map(PolicyRecord::from),
        )
    }

    #[tool(
        description = "Get a policy with the document of its default version (requires iam:GetPolicy)"
    )]
    pub(crate) async fn get_policy(
        &self,
        Parameters(input): Parameters<PolicyId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policies::Model::get_as(db, &auth, input.policy_id)
                .await
                .map(PolicyRecord::from),
        )
    }

    #[tool(
        description = "Delete an unattached tenant policy and its versions. Managed policies cannot be deleted."
    )]
    pub(crate) async fn delete_policy(
        &self,
        Parameters(input): Parameters<PolicyId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policies::Model::delete_as(db, &auth, input.policy_id)
                .await
                .map(|()| serde_json::json!({"deleted": true})),
        )
    }

    #[tool(
        description = "Add a version to a policy, which holds at most 5; managed policies cannot be versioned. A document holds Version 2012-10-17 and a Statement list; a statement has an optional Sid, an Effect of Allow or Deny, exactly one of Action or NotAction such as iam:* or iam:Get*, and a Resource of * or a tenant or workspace YRN; an explicit Deny beats every Allow (requires iam:CreatePolicyVersion)"
    )]
    pub(crate) async fn create_policy_version(
        &self,
        Parameters(input): Parameters<CreatePolicyVersionArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policy_versions::Model::create_as(
                db,
                &auth,
                input.policy_id,
                input.document,
                input.set_as_default,
            )
            .await
            .map(PolicyVersionRecord::from),
        )
    }

    #[tool(
        description = "Get one version of a policy; a managed policy has one virtual version whose id is the policy id (requires iam:GetPolicyVersion)"
    )]
    pub(crate) async fn get_policy_version(
        &self,
        Parameters(input): Parameters<PolicyVersionId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policy_versions::Model::get_as(
                db,
                &auth,
                input.policy_id,
                input.version_id,
            )
            .await
            .map(PolicyVersionRecord::from),
        )
    }

    #[tool(description = "List the versions of a policy (requires iam:ListPolicyVersions)")]
    pub(crate) async fn list_policy_versions(
        &self,
        Parameters(input): Parameters<PolicyId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policy_versions::Model::list_as(db, &auth, input.policy_id)
                .await
                .map(|versions| {
                    versions
                        .into_iter()
                        .map(PolicyVersionRecord::from)
                        .collect::<Vec<_>>()
                }),
        )
    }

    #[tool(
        description = "Make a version the default one, which evaluation uses (requires iam:SetDefaultPolicyVersion)"
    )]
    pub(crate) async fn set_default_policy_version(
        &self,
        Parameters(input): Parameters<PolicyVersionId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policy_versions::Model::set_default_as(
                db,
                &auth,
                input.policy_id,
                input.version_id,
            )
            .await
            .map(PolicyVersionRecord::from),
        )
    }

    #[tool(
        description = "Delete a version of a policy; the default version cannot be deleted (requires iam:DeletePolicyVersion)"
    )]
    pub(crate) async fn delete_policy_version(
        &self,
        Parameters(input): Parameters<PolicyVersionId>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        render(
            crate::models::iam_policy_versions::Model::delete_as(
                db,
                &auth,
                input.policy_id,
                input.version_id,
            )
            .await
            .map(|()| serde_json::json!({"deleted": true})),
        )
    }
}
