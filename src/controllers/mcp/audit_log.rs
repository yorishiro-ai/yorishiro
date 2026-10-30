use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::controllers::middleware::auth;
use crate::models::api_keys::ApiKeyScope;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuditLogArgs {
    /// One-based page number.
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_audit_log)]
impl YorishiroMcpServer {
    #[tool(
        meta = audit_requirement(),
        description = "List the selected workspace's paginated audit history (requires the independent audit grant; tenant or workspace administration does not grant audit access)"
    )]
    pub(crate) async fn list_audit_log(
        &self,
        Parameters(args): Parameters<AuditLogArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let key = super::extract_bearer_key(&parts)?;
        let headers = auth::header_pairs(&parts);
        let (actor, txn) = match auth::RequestAuth::new(self.app_context())
            .authorize_audit(key, &headers)
            .await
        {
            Ok(authorized) => authorized,
            Err(crate::error::YorishiroError::Unauthenticated) => {
                return Err(ErrorData::invalid_request("authentication failed", None));
            }
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        let page = match crate::models::pagination::ListParams::from_pagination_query(
            args.page.unwrap_or(1),
            args.page_size.unwrap_or(50),
        ) {
            Ok(page) => page,
            Err(error) => return Ok(err_to_tool_result(error)),
        };
        match crate::models::api_key_audit_log::list_for_workspace(&txn, actor.workspace_id, page)
            .await
        {
            Ok(records) => ok_json(crate::controllers::audit_log::render_records(
                records,
                crate::time::configured(self.app_context()),
            )),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}

fn audit_requirement() -> rmcp::model::MetaObject {
    let mut meta = super::scope_requirement(ApiKeyScope::Read);
    meta.0.insert(
        "yorishiro/required_grants".into(),
        serde_json::json!(["audit"]),
    );
    meta
}
