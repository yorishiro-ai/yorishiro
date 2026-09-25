mod entities;
mod import;
mod recall;
mod relations;
mod schemas;
mod search;
mod template_library;

use std::collections::HashSet;

use axum::http::request::Parts;
use loco_rs::app::AppContext;
use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ResultType, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ServerHandler, tool_handler};
use sea_orm::DatabaseTransaction;

use crate::controllers::extractors::{authenticator, db_handle};
use crate::db::AppContextBackend;
use crate::error::YorishiroError;
use crate::services::auth::{self, ApiKeyScope, AuthContext};

#[cfg(test)]
use rmcp::model::Tool;

/// Yorishiro MCP server, assembled from each edition's `#[tool_router]` implementations.
#[derive(Clone)]
pub struct YorishiroMcpServer {
    ctx: AppContext,
    tool_router: ToolRouter<Self>,
    enterprise_tool_names: HashSet<String>,
}

impl YorishiroMcpServer {
    pub(crate) fn new(
        ctx: AppContext,
        tool_router: ToolRouter<Self>,
        enterprise_tool_names: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            ctx,
            tool_router,
            enterprise_tool_names: enterprise_tool_names.into_iter().collect(),
        }
    }

    pub(crate) fn app_context(&self) -> &AppContext {
        &self.ctx
    }
}

/// The complete community-edition tool set.
pub(crate) fn community_tool_router() -> ToolRouter<YorishiroMcpServer> {
    compose_tool_routers([
        YorishiroMcpServer::tool_router_entities(),
        YorishiroMcpServer::tool_router_import(),
        YorishiroMcpServer::tool_router_recall(),
        YorishiroMcpServer::tool_router_relations(),
        YorishiroMcpServer::tool_router_schemas(),
        YorishiroMcpServer::tool_router_search(),
        YorishiroMcpServer::tool_router_template_library(),
    ])
}

pub(crate) fn compose_tool_routers(
    routers: impl IntoIterator<Item = ToolRouter<YorishiroMcpServer>>,
) -> ToolRouter<YorishiroMcpServer> {
    let mut composed = ToolRouter::new();
    let mut names = HashSet::new();
    for router in routers {
        for name in router.map.keys() {
            assert!(
                names.insert(name.to_string()),
                "duplicate MCP tool name: {}",
                name
            );
        }
        composed += router;
    }
    composed
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for YorishiroMcpServer {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !crate::services::edition::is_active(&self.ctx)
            && self.is_enterprise_tool(request.name.as_ref())
        {
            return Err(ErrorData::invalid_params("tool not found", None));
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
        if !crate::services::edition::is_active(&self.ctx) {
            tools.retain(|tool| !self.is_enterprise_tool(tool.name.as_ref()));
        }
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
        if !crate::services::edition::is_active(&self.ctx) && self.is_enterprise_tool(name) {
            return None;
        }
        self.tool_router.get(name).cloned()
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Yorishiro is a multi-tenant knowledge store with user-defined schemas. \
             Every tool call requires authentication via an `Authorization: Bearer <api-key>` \
             header, and the tools available depend on the API key's scope \
             (read/write/schema, where higher scopes include the permissions of lower ones).",
        )
    }
}

impl YorishiroMcpServer {
    fn is_enterprise_tool(&self, name: &str) -> bool {
        self.enterprise_tool_names.contains(name)
    }
}

/// Auth context plus a transaction with RLS already configured, held by calls that passed authentication and scope checks.
pub(crate) struct Authorized {
    ctx: AuthContext,
    txn: DatabaseTransaction,
}

impl Authorized {
    pub(super) fn txn(&self) -> &DatabaseTransaction {
        &self.txn
    }

    /// Commits the transaction.
    /// Every write handler must call this before returning `Ok`.
    pub(super) async fn commit(self) -> Result<(), ErrorData> {
        self.txn
            .commit()
            .await
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))
    }

    pub(crate) fn auth_context(&self) -> &AuthContext {
        &self.ctx
    }
}

/// `authorize` splits its outcome into two kinds rather than a single failure case: a protocol-level failure (`Err`) and a scope-insufficient business outcome (`Ok` variant).
/// The former is a dead end an agent can't usefully retry (missing/invalid API key); the latter is information an agent can act on.
///
/// Handlers match on this directly rather than through a macro, so the `return` that ends the call on a denial is visible where it happens.
/// It cannot collapse into the `Err` side of the handler's own `Result`: `rmcp`'s `ToolRouter` fixes every tool's function type to `Result<CallToolResponse, ErrorData>` (`rmcp-3.4.0`, `handler/server/router/tool.rs:202`), and `ErrorData` is the protocol-level failure, which is not what a denial is.
pub(crate) enum AuthzOutcome {
    Authorized(Authorized),
    ScopeDenied(CallToolResult),
}

/// A connection-less version of `Authorized`: only authenticates and verifies scope, without opening a transaction.
/// Tools that do slow work before touching the database (embedding generation) use this instead and call `TenantDb::acquire_for_workspace` afterward.
pub(super) enum VerifyOutcome {
    Verified(AuthContext),
    ScopeDenied(CallToolResult),
}

/// Copies the request's headers into the shape `auth::Authenticator` takes: the same thing the REST adapter does, so a replaced authenticator sees an MCP call exactly as it sees a REST one.
fn extract_bearer_key(parts: &Parts) -> Result<&str, ErrorData> {
    auth::extract_bearer_key(parts).ok_or_else(|| {
        ErrorData::invalid_request("missing or malformed Authorization header", None)
    })
}

/// The sole entry point for every tool handler.
/// Because there is no other way to obtain a `DatabaseTransaction`, forgetting the scope check is structurally impossible.
///
/// Shares `services::auth::authorize` with the REST adapter's `Authorized<R>` extractor; this just routes its result into the MCP protocol's two failure shapes (`ErrorData` at the protocol level, `CallToolResult` at the tool-result level).
pub(crate) async fn authorize(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<AuthzOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = auth::header_pairs(parts);

    // No DbHandle/Authenticator is built for SQLite (Hooks::after_context): this path authenticates
    // directly against ctx.db and opens a plain transaction, mirroring the REST adapter's
    // `Authorized<R>` SQLite extractor.
    if ctx.is_sqlite() {
        return match auth::authorize_sqlite(&ctx.db, presented_key, required).await {
            Ok((ctx, txn)) => Ok(AuthzOutcome::Authorized(Authorized { ctx, txn })),
            Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
                Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(err)))
            }
            Err(YorishiroError::Unauthenticated) => {
                Err(ErrorData::invalid_request("authentication failed", None))
            }
            Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
        };
    }

    let db = db_handle(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;
    let auth_impl =
        authenticator(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;

    match auth::authorize(&db, auth_impl.as_ref(), presented_key, required, &headers).await {
        Ok((ctx, txn)) => Ok(AuthzOutcome::Authorized(Authorized { ctx, txn })),
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
    }
}

/// Connection-less counterpart to `authorize`, used by tools that must run a slow operation (embedding generation) before touching the database.
/// See `services::auth::authorize_scope`.
pub(super) async fn verify(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<VerifyOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = auth::header_pairs(parts);

    // No DbHandle/Authenticator is built for SQLite (Hooks::after_context): this path authenticates
    // directly against ctx.db instead of going through the Authenticator trait, mirroring the REST
    // adapter's `AuthContext` extractor (extractors.rs) and `Verified<R>` extractor.
    if ctx.is_sqlite() {
        return match auth::authorize_scope_sqlite(&ctx.db, presented_key, required).await {
            Ok(ctx) => Ok(VerifyOutcome::Verified(ctx)),
            Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
                Ok(VerifyOutcome::ScopeDenied(err_to_tool_result(err)))
            }
            Err(YorishiroError::Unauthenticated) => {
                Err(ErrorData::invalid_request("authentication failed", None))
            }
            Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
        };
    }

    let db = db_handle(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;
    let auth_impl =
        authenticator(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;

    match auth::authorize_scope(&db, auth_impl.as_ref(), presented_key, required, &headers).await {
        Ok(ctx) => Ok(VerifyOutcome::Verified(ctx)),
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(VerifyOutcome::ScopeDenied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
    }
}

/// Converts a business-logic error into a tool call result (`is_error: true`).
/// `Internal` errors are logged with detail but only a generic message reaches the client, matching the REST adapter's `ApiError` policy.
pub(crate) fn err_to_tool_result(err: YorishiroError) -> CallToolResult {
    let message = match err {
        YorishiroError::Internal(err) => {
            tracing::error!(error = %err, "internal error in mcp tool handler");
            "internal server error".to_string()
        }
        YorishiroError::ValidationFailed {
            message,
            details,
            hint,
        } => {
            let mut msg = message;
            if !details.is_empty() {
                let detail_lines: Vec<String> = details
                    .iter()
                    .map(|d| format!("{}: {}", d.field, d.problem))
                    .collect();
                msg = format!("{msg}\n  {}", detail_lines.join("\n  "));
            }
            if !hint.is_empty() {
                msg = format!("{msg}\nhint: {hint}");
            }
            msg
        }
        other => other.to_string(),
    };
    CallToolResult::error(vec![ContentBlock::text(message)])
}

pub(crate) fn ok_json(value: impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value)
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

#[cfg(test)]
fn render_inventory_section(title: &str, tools: &[Tool]) -> String {
    let mut output = String::new();
    output.push_str("## ");
    output.push_str(title);
    output.push_str("\n\n");
    for tool in tools {
        output.push_str("### `");
        output.push_str(&tool.name);
        output.push_str("`\n\n");
        output.push_str(tool.description.as_deref().unwrap_or(""));
        output.push_str("\n\nInput schema:\n\n```json\n");
        output.push_str(
            &serde_json::to_string_pretty(&tool.schema_as_json_value())
                .expect("MCP tool schema is serializable"),
        );
        output.push_str("\n```\n\n");
    }
    output
}

#[cfg(test)]
pub(crate) fn render_inventory_fragment(community: &[Tool], enterprise: &[Tool]) -> String {
    let mut community = community.to_vec();
    let mut enterprise = enterprise.to_vec();
    community.sort_by(|a, b| a.name.cmp(&b.name));
    enterprise.sort_by(|a, b| a.name.cmp(&b.name));

    format!(
        "{}{}",
        render_inventory_section("Community tools", &community),
        render_inventory_section("Enterprise-only tools", &enterprise)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
    use rmcp::model::{CallToolResult, Tool};

    use super::{community_tool_router, compose_tool_routers, render_inventory_section};

    fn assert_inventory_contract(label: &str, tools: &[rmcp::model::Tool]) {
        let names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "{label} MCP tool names are not sorted: {names:?}"
        );
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "duplicate {label} MCP tool names: {names:?}"
        );
        println!("{label} MCP tools: {names:?}");

        for tool in tools {
            assert!(
                tool.description
                    .as_deref()
                    .is_some_and(|description| !description.trim().is_empty()),
                "{label} tool {} has no description",
                tool.name
            );
            let schema = tool.schema_as_json_value();
            assert_eq!(
                schema["type"], "object",
                "invalid {label} schema for {}",
                tool.name
            );
            assert!(
                jsonschema::meta::options().is_valid(&schema),
                "invalid {label} input schema for {}: {schema}",
                tool.name
            );
        }
    }

    #[test]
    fn community_inventory_contract_and_documentation_are_current() {
        let community = community_tool_router().list_all();
        assert_inventory_contract("community", &community);

        let docs = include_str!("../../../docs/en/mcp-tools.md");
        let start = "<!-- BEGIN GENERATED MCP INVENTORY -->\n";
        let end = "## Enterprise-only tools\n";
        let generated_community = docs
            .split_once(start)
            .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
            .expect("English MCP community inventory markers");
        assert_eq!(
            generated_community,
            render_inventory_section("Community tools", &community),
            "English MCP community documentation has drifted from the runtime inventory"
        );
    }

    #[test]
    fn composition_rejects_duplicate_registered_names_even_when_disabled() {
        fn router(name: &'static str) -> ToolRouter<super::YorishiroMcpServer> {
            ToolRouter::new().with_route(ToolRoute::new_dyn(
                Tool::new(name, "test tool", Arc::new(Default::default())),
                |_context| Box::pin(async { Ok(CallToolResult::default().into()) }),
            ))
        }

        let disabled = router("duplicate").with_disabled("duplicate");
        assert!(disabled.list_all().is_empty());
        assert!(disabled.map.contains_key("duplicate"));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compose_tool_routers([router("duplicate"), disabled]);
        }));
        assert!(
            result.is_err(),
            "duplicate registered names must be rejected"
        );
    }
}
