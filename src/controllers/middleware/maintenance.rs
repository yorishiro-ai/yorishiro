//! Refuses requests while the deployment is in maintenance.
//!
//! Layered in `Hooks::after_routes` so it covers REST and MCP alike, and reads the state on every request so that turning maintenance off takes effect at once.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::models::system_maintenance;

/// Paths that answer even under full lock.
///
/// `/_ping`, `/_health` and `/_readiness` are Loco's health-check routes.
/// Refusing them would make an orchestrator restart a server that is deliberately paused, and the state lives in the database, so a restart would not clear it.
fn always_served(path: &str) -> bool {
    matches!(path, "/_ping" | "/_health" | "/_readiness")
}

/// Whether the request intends to change something.
///
/// MCP performs its read-only check at tool dispatch, where the operation is known.
/// Full lock still applies here to both transports.
fn is_write(request: &Request) -> bool {
    if request.uri().path() == "/mcp" {
        return false;
    }
    !matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS")
}

pub(crate) async fn maintenance_guard(
    State(ctx): State<AppContext>,
    request: Request,
    next: Next,
) -> Response {
    if always_served(request.uri().path()) {
        return next.run(request).await;
    }

    let current = match system_maintenance::get(&ctx.db).await {
        Ok(current) => current,
        Err(err) => return ApiError::from(err).into_response(),
    };

    match current.refusal(is_write(&request)) {
        Some(err) => {
            let retry_after = current.retry_after;
            let mut response = ApiError::from(err).into_response();
            // Agents retry on the header rather than on the body.
            // Without it a 423 invites an immediate retry, which is the load the mode exists to shed.
            if let Ok(value) = retry_after.to_string().parse() {
                response.headers_mut().insert("retry-after", value);
            }
            response
        }
        None => next.run(request).await,
    }
}
