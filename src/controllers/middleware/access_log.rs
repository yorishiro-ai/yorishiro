use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};
use loco_rs::{
    app::AppContext,
    controller::middleware::request_id::{LocoRequestId, request_id_middleware},
    environment::Environment,
};
use tracing::Instrument;

/// The request target used by the access log.
/// Query strings are deliberately excluded because callback parameters can carry codes, state, and provider error details.
pub fn path_only(uri: &axum::http::Uri) -> &str {
    uri.path()
}

pub(crate) fn layer(mut router: axum::Router, ctx: &AppContext) -> axum::Router {
    let config = &ctx.config.server.middlewares;
    if config.logger.as_ref().is_none_or(|logger| logger.enable) {
        router = router
            .layer(axum::middleware::from_fn(log_request))
            .layer(axum::Extension(ctx.environment.clone()));
    }
    if config.request_id.as_ref().is_none_or(|id| id.enable) {
        router = router.layer(axum::middleware::from_fn(request_id_middleware));
    }
    router
}

async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = path_only(request.uri()).to_owned();
    let version = request.version();
    let user_agent = request
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let request_id = request
        .extensions()
        .get::<LocoRequestId>()
        .map_or_else(|| "req-id-none".to_string(), |id| id.get().to_string());
    let environment = request
        .extensions()
        .get::<Environment>()
        .map_or_default(ToString::to_string);
    let started = Instant::now();
    let span = tracing::error_span!(
        "http-request",
        "http.method" = tracing::field::display(&method),
        "http.uri" = tracing::field::display(&path),
        "http.version" = tracing::field::debug(version),
        "http.user_agent" = tracing::field::display(&user_agent),
        "environment" = tracing::field::display(&environment),
        request_id = tracing::field::display(&request_id),
        caller = tracing::field::Empty,
        workspace = tracing::field::Empty,
    );
    async move {
        let response = next.run(request).await;
        tracing::info!(
            http_status = response.status().as_u16(),
            latency_ms = started.elapsed().as_secs_f64() * 1000.0,
            "http request completed",
        );
        response
    }
    .instrument(span)
    .await
}
