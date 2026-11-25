//! The MCP server type and its tool dispatch.

use super::{
    AppContext, Arc, CallToolRequestParams, CallToolResponse, ErrorData, ListToolsResult,
    McpToolPolicy, PaginatedRequestParams, RequestContext, ResultType, RoleServer,
    ServerCapabilities, ServerConfig, ServerHandler, ToolRouter, err_to_tool_result, tool_handler,
};

/// Yorishiro MCP server assembled from registered `#[tool_router]` implementations.
#[derive(Clone)]
pub struct YorishiroMcpServer {
    ctx: AppContext,
    tool_router: ToolRouter<Self>,
    tool_policy: Arc<dyn McpToolPolicy>,
}

impl YorishiroMcpServer {
    fn accepts_workspace_selection(&self, tool: &rmcp::model::Tool) -> bool {
        self.tool_policy.accepts_workspace_selection(&tool.name)
            && tool
                .input_schema
                .get("properties")
                .is_none_or(|properties| properties.get("workspace_id").is_none())
    }

    fn advertised_tool(&self, mut tool: rmcp::model::Tool) -> rmcp::model::Tool {
        if !self.accepts_workspace_selection(&tool) {
            return tool;
        }
        let schema = Arc::make_mut(&mut tool.input_schema);
        let properties = schema
            .entry("properties")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(properties) = properties.as_object_mut() {
            properties.entry("workspace_id").or_insert_with(|| serde_json::json!({
                "type":"string", "format":"uuid",
                "description":"Workspace to operate on for this call. No workspace-bound key is required."
            }));
        }
        tool
    }

    pub(crate) fn new(
        ctx: AppContext,
        tool_router: ToolRouter<Self>,
        tool_policy: Arc<dyn McpToolPolicy>,
    ) -> Self {
        Self {
            ctx,
            tool_router,
            tool_policy,
        }
    }

    pub(crate) const fn app_context(&self) -> &AppContext {
        &self.ctx
    }

    /// Authenticates the request; IAM authorizes each operation afterwards.
    pub(crate) async fn verify(
        &self,
        parts: &axum::http::request::Parts,
    ) -> Result<super::VerifyOutcome, ErrorData> {
        super::authorize::verify(self.app_context(), parts).await
    }

    pub(crate) fn tool_capabilities(
        &self,
        audit: bool,
    ) -> Result<Vec<serde_json::Value>, ErrorData> {
        self.tool_router
            .list_all()
            .into_iter()
            .filter(|tool| self.tool_policy.allows(&self.ctx, tool.name.as_ref()))
            .map(|tool| {
                let grants = tool.meta.as_ref()
                    .and_then(|meta| meta.0.get("yorishiro/required_grants"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]));
                Ok(serde_json::json!({
                    "name": tool.name,
                    "required_grants": grants,
                    "grants_satisfied": grants.as_array().is_some_and(|grants| grants.iter().all(|grant| grant == "audit" && audit)),
                }))
            })
            .collect()
    }
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for YorishiroMcpServer {
    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !self.tool_policy.allows(&self.ctx, request.name.as_ref()) {
            return Err(ErrorData::invalid_params("tool not found", None));
        }
        if self
            .tool_router
            .get(&request.name)
            .is_some_and(|tool| self.accepts_workspace_selection(tool))
            && let Some(workspace) = request
                .arguments
                .as_mut()
                .and_then(|args| args.remove("workspace_id"))
        {
            let workspace = workspace
                .as_str()
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
                .ok_or_else(|| ErrorData::invalid_params("workspace_id must be a UUID", None))?;
            if let Some(parts) = context.extensions.get_mut::<axum::http::request::Parts>() {
                let value = axum::http::HeaderValue::from_str(&workspace.to_string())
                    .map_err(crate::error::mcp_internal)?;
                if parts
                    .headers
                    .get_all(crate::models::api_keys::WORKSPACE_HEADER)
                    .iter()
                    .any(|header| {
                        header
                            .to_str()
                            .ok()
                            .and_then(|header| uuid::Uuid::parse_str(header.trim()).ok())
                            != Some(workspace)
                    })
                {
                    return Err(ErrorData::invalid_params(
                        "workspace_id conflicts with x-workspace-id",
                        None,
                    ));
                }
                parts
                    .headers
                    .insert(crate::models::api_keys::WORKSPACE_HEADER, value);
            }
        }
        let maintenance = match crate::models::system_maintenance::get(&self.ctx.db).await {
            Ok(state) => state,
            Err(error) => return Ok(err_to_tool_result(error).into()),
        };
        if let Some(error) = maintenance.refusal(!self.tool_policy.read_only(request.name.as_ref()))
        {
            return Ok(err_to_tool_result(error).into());
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send {
        let supports_cache_hints = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2026_07_28);
        let mut tools: Vec<_> = self
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| self.advertised_tool(tool))
            .collect();
        tools.retain(|tool| self.tool_policy.allows(&self.ctx, tool.name.as_ref()));
        std::future::ready(Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools,
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(rmcp::model::CacheScope::Public),
        }))
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        if !self.tool_policy.allows(&self.ctx, name) {
            return None;
        }
        self.tool_router
            .get(name)
            .cloned()
            .map(|tool| self.advertised_tool(tool))
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Yorishiro is a multi-tenant knowledge store with user-defined schemas. \
             Every tool call requires authentication via an `Authorization: Bearer <api-key>` \
             header, and IAM policies decide which operations a key's user may perform. \
             Select a workspace with the `workspace_id` tool argument or `X-Workspace-Id` header. \
             Without selection, lists and search span permitted workspaces, resource IDs resolve their workspace, \
             and destination operations select a workspace only when exactly one is permitted. \
             Tenant root sign-in sessions have full tenant access; issued keys retain their grants and bindings.",
        )
    }
}
