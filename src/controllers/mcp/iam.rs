use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use super::YorishiroMcpServer;
use crate::dtos::iam::{
    ApiEventRecord, ListApiEvents, SimulatePrincipalPolicyRequest, SimulatePrincipalPolicyResponse,
};
use crate::models::pagination::ListParams;

#[tool_router(vis = "pub(crate)", router = tool_router_iam)]
impl YorishiroMcpServer {
    #[tool(
        description = "List the tenant's recorded REST and MCP calls: who, when, from where, what and the outcome. Requires audit:ListApiEvents."
    )]
    pub(crate) async fn list_api_events(
        &self,
        Parameters(input): Parameters<ListApiEvents>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        render(
            crate::models::api_events::Model::list_as(
                &self.app_context().db,
                &auth,
                ListParams::from(&input),
                crate::models::api_events::Filter {
                    request_id: input.request_id.as_deref(),
                    correlation_id: input.correlation_id.as_deref(),
                    resource: input.resource.as_deref(),
                },
            )
            .await
            .map(|events| {
                events
                    .into_iter()
                    .map(ApiEventRecord::from)
                    .collect::<Vec<_>>()
            }),
        )
    }

    #[tool(
        description = "Evaluate whether the policies of a user allow an action on a resource, with the code real requests use; the user defaults to the caller and the resource to the tenant or every workspace. Returns allowed, implicit_deny or explicit_deny and the names of the matching policies (requires iam:SimulatePrincipalPolicy)"
    )]
    pub(crate) async fn simulate_principal_policy(
        &self,
        Parameters(input): Parameters<SimulatePrincipalPolicyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let db = &self.app_context().db;
        let action = match input.action() {
            Ok(action) => action,
            Err(error) => return Ok(super::err_to_tool_result(error)),
        };
        render(
            crate::models::iam::access::simulate_principal_policy(
                db,
                &auth,
                input.user_id,
                action,
                input.resource.as_deref(),
            )
            .await
            .map(SimulatePrincipalPolicyResponse::from),
        )
    }
}
