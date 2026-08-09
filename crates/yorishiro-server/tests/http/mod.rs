use super::*;

/// This module groups the HTTP layer's submodules. The test pins that grouping: `controllers`,
/// `mcp` and `middleware` are what `routes.rs` and downstream crates reach through, so losing
/// one would break those call sites rather than this file.
#[test]
fn the_http_layer_exposes_its_three_submodules() {
    let _: fn(
        std::sync::Arc<middleware::rate_limit::RateLimiter>,
    ) -> axum::Router<crate::state::AppState> = controllers::router;
    let _ = std::any::type_name::<mcp::YorishiroMcpServer>();
    let _ = std::any::type_name::<middleware::rate_limit::RateLimiter>();
}
