use crate::controllers::mcp::{try_tool, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::model::CallToolResult;
use rmcp::model::ContentBlock;
use rmcp::{tool, tool_router};

use super::YorishiroMcpServer;
use crate::models::api_keys::ApiKeyScope;
use crate::models::transfer::export;

#[tool_router(vis = "pub(crate)", router = tool_router_export)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Export schemas, entities, and relations in the selected workspace as newline-delimited JSON (requires read scope and export IAM permission); one {\"kind\":\"schema\"|\"entity\"|\"relation\",\"record\":{...}} object per line, the same format import_jsonl reads back. Limited to 8 MiB and 10,000 records; larger workspaces must use paginated list APIs."
    )]
    pub(crate) async fn export_jsonl(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let required = self.required_scope("export_jsonl")?;
        let actor = verified!(super::verify(self.app_context(), &parts, required).await?);
        let mut authorized = try_tool!(
            super::authorize::Authorized::for_target(
                self.app_context(),
                &actor,
                required,
                crate::models::iam::access::WorkspaceTarget::Permission(
                    crate::models::iam::policy::Action::Export,
                ),
            )
            .await
        );
        try_tool!(
            authorized
                .require_action(crate::models::iam::policy::Action::Export)
                .await
        );

        let lines =
            try_tool!(export::export_jsonl(authorized.txn(), authorized.workspace_id()).await);
        Ok(CallToolResult::success(vec![ContentBlock::text(lines)]))
    }
}
