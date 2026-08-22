use axum::Router;
use loco_rs::app::AppContext;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

use crate::services::mcp::YorishiroMcpServer;

/// Mounts the MCP server under `/mcp`.
///
/// `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not a Loco `Routes`/axum
/// handler function, so it can't go through `Hooks::routes()`/`AppRoutes` like the REST
/// controllers: it's mounted via `Router::nest_service` in `Hooks::after_routes` instead, which
/// is the hook Loco itself offers for exactly this (custom Axum logic after Loco's own routes
/// are built).
pub fn mount(router: Router, ctx: &AppContext) -> Router {
    let ctx = ctx.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(YorishiroMcpServer::new(ctx.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    router.nest_service("/mcp", mcp_service)
}
