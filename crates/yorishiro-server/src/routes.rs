use axum::extract::DefaultBodyLimit;
use axum::{Router, routing::get};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::http::controllers::{health, whoami};
use crate::http::{controllers, mcp};
use crate::state::AppState;

/// The routing configuration itself needs to be identical between `main` and the
/// integration tests, so it's factored into a function that builds the app from just an
/// `AppState`. `static_fallback` is mounted for any path no API route matches, which is what
/// serves the SPA and lets it own client-side routing.
///
/// The caller supplies that fallback rather than this crate reaching for one: the SPA is part of
/// the paid edition under `ee/`, and a crate here must not depend on that direction. A caller
/// with no UI to serve can pass any `MethodRouter`: the tests pass one that always 404s.
///
/// **Merging your own routes in.** `axum::Router::merge` does not propagate a `.layer()` from
/// either side to the other, so routes added via `some_router.merge(build_app(state,
/// static_fallback))` get none of this router's `DefaultBodyLimit`/rate-limit/CORS/trace-id layers
/// unless applied to `some_router` directly, *before* merging. This crate factors those layers
/// out for exactly that reason:
///
/// ```ignore
/// // `my_unauthenticated_routes` must already be `Router<()>` (call `.with_state(...)` on it
/// // first if it was built as `Router<MyState>`): `apply_observability_layers` and
/// // `apply_body_limit_layer` both take/return `Router` (i.e. `Router<()>`), matching what
/// // `build_app`/`build_app_with_rate_limiter` themselves return.
/// let rate_limiter = std::sync::Arc::new(RateLimiter::from_env());
/// let my_routes = apply_body_limit_layer(apply_observability_layers(
///     apply_rate_limit_layer(my_unauthenticated_routes, rate_limiter.clone()),
/// )); // only routes that should be reachable without a bearer token need the rate limiter
/// let app = my_routes.merge(build_app_with_rate_limiter(state, static_fallback, rate_limiter));
/// ```
///
/// (`RateLimiter`/`apply_rate_limit_layer` are in
/// `crate::http::middleware::rate_limit`; `apply_body_limit_layer`/`apply_observability_layers`
/// are in this module.) Pass the same `Arc<RateLimiter>` to both sides to share one quota
/// across this crate's `/auth/*`/`/setup*` routes and your own unauthenticated routes (e.g. an
/// OAuth login/callback pair): see `build_app_with_rate_limiter`.
pub fn build_app(state: AppState, static_fallback: axum::routing::MethodRouter) -> Router {
    build_app_with_rate_limiter(
        state,
        static_fallback,
        std::sync::Arc::new(crate::http::middleware::rate_limit::RateLimiter::from_env()),
    )
}

/// Like [`build_app`], but takes the `Arc<RateLimiter>` protecting this crate's own `/auth/signup`, `/auth/login`, `/setup`, `/setup/status` routes instead of constructing one internally: for a downstream crate that wants to share that same quota with its own unauthenticated routes (see [`build_app`]'s doc comment for the full pattern).
pub fn build_app_with_rate_limiter(
    state: AppState,
    static_fallback: axum::routing::MethodRouter,
    rate_limiter: std::sync::Arc<crate::http::middleware::rate_limit::RateLimiter>,
) -> Router {
    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(mcp::YorishiroMcpServer::new(state.clone()))
        },
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let router = Router::new()
        .route("/up", get(health::up_check))
        .route("/health", get(health::health_check))
        .route("/whoami", get(whoami::whoami))
        .nest_service("/mcp", mcp_service)
        .merge(controllers::router(rate_limiter))
        .merge(
            SwaggerUi::new("/docs").url("/api-docs/openapi.json", controllers::ApiDoc::openapi()),
        );
    // Before with_state so the guard sees AppState, and inside build_app so every route this process serves is covered: REST, MCP and the wizard alike.
    // /up and /health opt out inside the guard itself.
    let router = router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::http::middleware::maintenance::maintenance_guard,
    ));
    let router = apply_body_limit_layer(router).with_state(state);

    apply_observability_layers(router).fallback_service(static_fallback)
}

/// Applies the 2 MiB request-body cap every route in this process needs.
/// Factored out for the same reason as `apply_observability_layers`: see that function's doc comment and `build_app`'s "Merging your own routes in" section.
pub fn apply_body_limit_layer<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(DefaultBodyLimit::max(2 * 1024 * 1024))
}

/// Applies the CORS / request-id / access-log stack that every API route in this process needs.
/// Factored out of `build_app` so a process embedding this server alongside its own routes can apply the same stack to its own sub-router *before* merging it with `build_app`'s: `axum::Router::merge` doesn't propagate layers from either side to the other, so each sub-router must carry its own copy of this stack for every route to get it exactly once.
/// Not applied to `build_app`'s static-asset fallback (added after this runs), which is deliberately left untraced.
pub fn apply_observability_layers(router: Router) -> Router {
    router
        .layer(build_cors_layer())
        // Copies the resolved `x-request-id` onto the response so a caller or proxy can correlate its request with this server's logs.
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            // The default span/response levels are DEBUG, which a production `RUST_LOG=info` silently drops: raised to INFO so the access log (method, path, status, latency) actually reaches whichever target `logging::init` selected.
            // The span carries `request_id` so any warn/error emitted while handling a request correlates with its access-log line.
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::extract::Request| {
                    let request_id = request
                        .headers()
                        .get("x-request-id")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default();
                    tracing::info_span!(
                        "request",
                        %request_id,
                        method = %request.method(),
                        uri = %request.uri(),
                    )
                })
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        // Generates an `x-request-id` (UUID) when the incoming request lacks one.
        // Added last so it is the outermost layer and runs before the trace span above reads the header.
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

fn build_cors_layer() -> CorsLayer {
    let origins_str = std::env::var("YORISHIRO_CORS_ORIGINS").unwrap_or_default();

    let layer = if !origins_str.is_empty() {
        let origins: Vec<_> = origins_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        CorsLayer::new().allow_origin(origins)
    } else if cfg!(debug_assertions) {
        // Debug builds only: with no explicit YORISHIRO_CORS_ORIGINS, allow any localhost/127.0.0.1 port so browser-based dev tools (e.g. the MCP Inspector) can reach this server without requiring a manually configured origin list.
        // Release builds never take this branch, so the all-reject default (below) is unaffected in production.
        debug_local_origin_layer()
    } else {
        CorsLayer::new()
    };

    layer
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(AllowHeaders::list([
            "authorization".parse().unwrap(),
            "content-type".parse().unwrap(),
        ]))
        .expose_headers(["x-request-id".parse().unwrap()])
}

/// Matches `http://localhost:<any port>` and `http://127.0.0.1:<any port>` origins.
/// Only reached from a debug build with `YORISHIRO_CORS_ORIGINS` unset (see `build_cors_layer`).
fn debug_local_origin_layer() -> CorsLayer {
    CorsLayer::new().allow_origin(AllowOrigin::predicate(|origin, _parts| {
        origin
            .to_str()
            .map(|s| s.starts_with("http://localhost:") || s.starts_with("http://127.0.0.1:"))
            .unwrap_or(false)
    }))
}

#[cfg(test)]
#[path = "../tests/routes.rs"]
mod tests;
