mod api_keys;
mod audit_log;
mod authorize;
pub mod entities;
mod export;
mod iam;
mod import;
mod members;
pub(crate) mod policy;
mod recall;
pub mod relations;
mod schemas;
mod search;
mod server;
mod template_library;
mod tenants;
mod users;
mod whoami;
mod workspaces;

use std::collections::HashSet;
use std::sync::Arc;

use axum::Router;
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
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, tool_handler};
use sea_orm::DatabaseTransaction;

use crate::controllers::middleware::auth;
use crate::error::YorishiroError;
use crate::models::api_keys::{ApiKeyScope, AuthContext};

pub(crate) use authorize::{AuthzOutcome, VerifyOutcome, authorize, verify};
use authorize::{extract_bearer_key, header_error};
pub use policy::{McpToolPolicy, McpToolSet};
pub use server::{YorishiroMcpServer, scope_requirement};

/// The complete community-edition tool set.
pub fn community_tool_router() -> ToolRouter<YorishiroMcpServer> {
    compose_tool_routers([
        YorishiroMcpServer::tool_router_entities(),
        YorishiroMcpServer::tool_router_export(),
        YorishiroMcpServer::tool_router_import(),
        YorishiroMcpServer::tool_router_members(),
        YorishiroMcpServer::tool_router_recall(),
        YorishiroMcpServer::tool_router_relations(),
        YorishiroMcpServer::tool_router_schemas(),
        YorishiroMcpServer::tool_router_search(),
        YorishiroMcpServer::tool_router_template_library(),
        YorishiroMcpServer::tool_router_tenants(),
        YorishiroMcpServer::tool_router_iam(),
        YorishiroMcpServer::tool_router_whoami(),
        YorishiroMcpServer::tool_router_workspaces(),
        YorishiroMcpServer::tool_router_api_keys(),
        YorishiroMcpServer::tool_router_audit_log(),
        YorishiroMcpServer::tool_router_users(),
    ])
}

///
/// # Panics
/// Panics if an internal invariant required by this operation is violated.
pub fn compose_tool_routers(
    routers: impl IntoIterator<Item = ToolRouter<YorishiroMcpServer>>,
) -> ToolRouter<YorishiroMcpServer> {
    let mut composed = ToolRouter::new();
    let mut names = HashSet::new();
    for router in routers {
        for name in router.map.keys() {
            assert!(
                names.insert(name.to_string()),
                "duplicate MCP tool name: {name}"
            );
        }
        composed += router;
    }
    composed
}

/// Converts a business-logic error into a tool call result (`is_error: true`).
/// `Internal` errors are logged with detail but only a generic message reaches the client, matching the REST adapter's `ApiError` policy.
pub(crate) fn err_to_tool_result(err: YorishiroError) -> CallToolResult {
    let (_, body) = err.into_http_parts();
    let mut message = body["error"]["message"]
        .as_str()
        .unwrap_or("request failed")
        .to_owned();
    if let Some(details) = body["error"]["details"].as_array() {
        for detail in details {
            if let (Some(field), Some(problem)) =
                (detail["field"].as_str(), detail["problem"].as_str())
            {
                message.push_str(&format!("\n  {field}: {problem}"));
            }
        }
    }
    if let Some(hint) = body["error"]["hint"]
        .as_str()
        .filter(|hint| !hint.is_empty())
    {
        message.push_str(&format!("\nhint: {hint}"));
    }
    let mut result = CallToolResult::error(vec![ContentBlock::text(message)]);
    result.structured_content = Some(body);
    result
}

pub(crate) fn ok_json(value: impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value).map_err(crate::error::mcp_internal)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Mounts the MCP server under `/mcp`.
///
/// `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not a Loco `Routes`/axum handler function, so it can't go through `Hooks::routes()`/`AppRoutes` like the REST controllers: it's mounted via `Router::nest_service` in `Hooks::after_routes` instead.
///
/// # Panics
/// Panics if application settings cannot be decoded.
pub fn mount<F>(router: Router, ctx: &AppContext, server_factory: F) -> Router
where
    F: Fn(AppContext) -> YorishiroMcpServer + Clone + Send + Sync + 'static,
{
    let ctx = ctx.clone();
    let transport_config = transport_config(&ctx.config).expect("invalid MCP configuration");
    let mcp_service = StreamableHttpService::new(
        move || Ok(server_factory(ctx.clone())),
        LocalSessionManager::default().into(),
        transport_config,
    );

    router.nest("/mcp", Router::new().fallback_service(mcp_service))
}

/// Derives MCP HTTP security settings from the server's shared network configuration.
///
/// # Errors
/// Returns an error if application settings cannot be decoded.
pub fn transport_config(
    config: &loco_rs::config::Config,
) -> loco_rs::Result<StreamableHttpServerConfig> {
    let settings = config.settings::<crate::data::settings::Settings>()?;
    let mut transport = StreamableHttpServerConfig::default().enforce_origin_validation();
    let binding = config.server.binding.as_str();
    if binding
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_unspecified())
    {
        tracing::warn!(
            binding,
            "MCP accepts any Host on an explicitly public listener; use a Host-validating reverse proxy"
        );
        transport = transport.disable_allowed_hosts();
    } else {
        transport.allowed_hosts.push(binding.to_owned());
        transport.allowed_hosts.extend(comma_separated(
            settings.mcp.allowed_hosts.as_deref().unwrap_or_default(),
        ));
    }
    if let Some(cors) = config
        .server
        .middlewares
        .cors
        .as_ref()
        .filter(|cors| cors.enable)
    {
        // rmcp treats portless entries as any port, unlike browser CORS's exact origins.
        for origin in &cors.allow_origins {
            let uri = origin.parse::<axum::http::Uri>().map_err(|error| {
                loco_rs::Error::Message(format!("invalid allowed origin: {error}"))
            })?;
            let port = match uri.scheme_str() {
                Some("https") => 443,
                _ => 80,
            };
            transport.allowed_origins.push(if uri.port_u16().is_none() {
                format!("{origin}:{port}")
            } else {
                origin.clone()
            });
        }
    }
    Ok(transport)
}

fn comma_separated(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
