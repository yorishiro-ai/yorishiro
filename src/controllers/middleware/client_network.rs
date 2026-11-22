use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use ipnet::IpNet;

pub(crate) async fn require_allowed_network(
    State(networks): State<Arc<[IpNet]>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let protected = ["/api", "/mcp", "/docs", "/auth", "/setup"]
        .iter()
        .any(|prefix| {
            path.strip_prefix(prefix)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
        });
    // Forwarding headers are untrusted; behind a proxy this sees the proxy's IP.
    if protected
        && !request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .is_some_and(|ConnectInfo(peer)| {
                let address = peer.ip();
                networks.iter().any(|network| {
                    network.contains(&address) || network.contains(&address.to_canonical())
                })
            })
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}
