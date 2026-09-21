use axum::Router;
use loco_rs::app::AppContext;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

use crate::services::mcp::YorishiroMcpServer;

/// Mounts the MCP server under `/mcp`.
///
/// `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not a Loco `Routes`/axum handler function, so it can't go through `Hooks::routes()`/`AppRoutes` like the REST controllers: it's mounted via `Router::nest_service` in `Hooks::after_routes` instead.
pub fn mount<F>(router: Router, ctx: &AppContext, server_factory: F) -> Router
where
    F: Fn(AppContext) -> YorishiroMcpServer + Clone + Send + Sync + 'static,
{
    let ctx = ctx.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(server_factory(ctx.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    router.nest_service("/mcp", mcp_service)
}
