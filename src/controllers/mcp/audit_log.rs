use crate::controllers::mcp::{render, try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::YorishiroMcpServer;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuditLogArgs {
    /// One-based page number.
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_audit_log)]
impl YorishiroMcpServer {
    #[tool(description = "List workspace audit events allowed by audit:ListWorkspaceAuditEvents.")]
    pub(crate) async fn list_workspace_audit_events(
        &self,
        Parameters(args): Parameters<AuditLogArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        let page = try_tool!(
            crate::models::pagination::ListParams::from_pagination_query(
                args.page.unwrap_or(1),
                args.page_size.unwrap_or(50),
            )
        );
        let ctx = self.app_context();
        render(
            crate::models::access_key_audit_log::list_for_actor(ctx, &actor, page)
                .await
                .and_then(|records| {
                    crate::controllers::audit_log::render_records(
                        records,
                        crate::data::time::configured(ctx),
                    )
                }),
        )
    }
}
