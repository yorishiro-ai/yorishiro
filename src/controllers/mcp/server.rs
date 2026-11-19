//! The MCP server type, its tool dispatch and the scope metadata it advertises.

use super::*;

/// Yorishiro MCP server assembled from registered `#[tool_router]` implementations.
#[derive(Clone)]
pub struct YorishiroMcpServer {
    ctx: AppContext,
    tool_router: ToolRouter<Self>,
    tool_policy: Arc<dyn McpToolPolicy>,
}

impl YorishiroMcpServer {
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

    pub(crate) fn app_context(&self) -> &AppContext {
        &self.ctx
    }

    pub(crate) fn required_scope(&self, name: &str) -> Result<ApiKeyScope, ErrorData> {
        self.tool_router
            .get(name)
            .and_then(|tool| tool.meta.as_ref())
            .and_then(|meta| meta.0.get("yorishiro/required_scope"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .ok_or_else(|| ErrorData::internal_error("tool scope requirement missing", None))
    }

    pub(crate) fn tool_capabilities(
        &self,
        scopes: &[ApiKeyScope],
        audit: bool,
    ) -> Result<Vec<serde_json::Value>, ErrorData> {
        self.tool_router
            .list_all()
            .into_iter()
            .filter(|tool| self.tool_policy.allows(&self.ctx, tool.name.as_ref()))
            .map(|tool| {
                let required = self.required_scope(tool.name.as_ref())?;
                let grants = tool.meta.as_ref()
                    .and_then(|meta| meta.0.get("yorishiro/required_grants"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]));
                Ok(serde_json::json!({
                    "name": tool.name,
                    "required_scope": required,
                    "scope_satisfied": scopes.contains(&required),
                    "required_grants": grants,
                    "grants_satisfied": grants.as_array().is_some_and(|grants| grants.iter().all(|grant| grant == "audit" && audit)),
                }))
            })
            .collect()
    }
}

/// Declares the scope used by both tool authorization and capability discovery.
pub fn scope_requirement(scope: ApiKeyScope) -> rmcp::model::MetaObject {
    let mut meta = rmcp::model::MetaObject::new();
    meta.0
        .insert("yorishiro/required_scope".into(), serde_json::json!(scope));
    meta
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for YorishiroMcpServer {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !self.tool_policy.allows(&self.ctx, request.name.as_ref()) {
            return Err(ErrorData::invalid_params("tool not found", None));
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

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let supports_cache_hints = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2026_07_28);
        let mut tools = self.tool_router.list_all();
        tools.retain(|tool| self.tool_policy.allows(&self.ctx, tool.name.as_ref()));
        Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools,
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(rmcp::model::CacheScope::Public),
        })
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        if !self.tool_policy.allows(&self.ctx, name) {
            return None;
        }
        self.tool_router.get(name).cloned()
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Yorishiro is a multi-tenant knowledge store with user-defined schemas. \
             Every tool call requires authentication via an `Authorization: Bearer <api-key>` \
             header, and the tools available depend on the API key's scope \
             (read/write/schema, where higher scopes include the permissions of lower ones). \
             Tenant-scoped keys require `X-Workspace-Id` for workspace operations; tenant operations do not select a workspace.",
        )
    }
}
