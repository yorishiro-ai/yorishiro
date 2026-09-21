use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};
use loco_rs::controller::middleware::{MiddlewareLayer, logger};
use serde::Serialize;
use tracing::Instrument;

/// The request target used by the access log.
/// Query strings are deliberately excluded because OAuth callback parameters carry codes, state, and provider error details.
pub fn path_only(uri: &axum::http::Uri) -> &str {
    uri.path()
}

#[derive(Debug, Clone, Serialize)]
pub struct Middleware {
    enable: bool,
}

impl Middleware {
    pub fn new(config: &logger::Config) -> Self {
        Self {
            enable: config.enable,
        }
    }
}

impl MiddlewareLayer for Middleware {
    fn name(&self) -> &'static str {
        "logger"
    }

    fn is_enabled(&self) -> bool {
        self.enable
    }

    fn config(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::to_value(self)
    }

    fn apply(
        &self,
        app: axum::Router<loco_rs::app::AppContext>,
    ) -> loco_rs::Result<axum::Router<loco_rs::app::AppContext>> {
        Ok(app.layer(axum::middleware::from_fn(log_request)))
    }
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
        .get::<loco_rs::controller::middleware::request_id::LocoRequestId>()
        .map_or_else(|| "req-id-none".to_string(), |id| id.get().to_string());
    let environment = request
        .extensions()
        .get::<loco_rs::environment::Environment>()
        .map(ToString::to_string)
        .unwrap_or_default();
    let started = Instant::now();
    let span = tracing::error_span!(
        "http-request",
        "http.method" = tracing::field::display(&method),
        "http.uri" = tracing::field::display(&path),
        "http.version" = tracing::field::debug(version),
        "http.user_agent" = tracing::field::display(&user_agent),
        "environment" = tracing::field::display(&environment),
        request_id = tracing::field::display(&request_id),
    );
    let response = next.run(request).instrument(span).await;
    tracing::info!(
        http_method = %method,
        http_uri = %path,
        http_status = response.status().as_u16(),
        latency_ms = started.elapsed().as_secs_f64() * 1000.0,
        user_agent = %user_agent,
        environment = %environment,
        request_id = %request_id,
        "http request completed",
    );
    response
}
