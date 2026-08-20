//! Refuses requests while the deployment is in maintenance.
//!
//! Runs before routing so it covers REST and MCP alike, and reads the state per request rather than caching it: an operator turning maintenance off expects the next request to be served, not the one after some TTL.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use yorishiro_core::models::maintenance;

use crate::error::ApiError;
use crate::state::AppState;

/// Paths that answer even under full lock.
///
/// `/up` and `/health` are how an orchestrator decides whether the process is alive.
/// Refusing them would have the scheduler restart a server that is deliberately paused, and restarting it would not clear the state, which lives in the database, so the loop would not converge.
///
/// `/api/system/maintenance` is the switch itself.
/// Behind the guard, a full lock entered over REST could only be left over the CLI, making the endpoint a one-way door for anyone without shell access to the host.
/// Being served is not being open: reaching the handler still needs a `migration`-scoped key, exactly as it does when the deployment is serving normally.
fn always_served(path: &str) -> bool {
    matches!(path, "/up" | "/health" | "/api/system/maintenance")
}

/// Whether the request intends to change something.
///
/// Decided by method: `POST` to `/mcp` may be a read, but the middleware cannot know which tool without parsing the body, and a body consumed here is a body the handler no longer has.
/// Read-only mode therefore refuses MCP wholesale, which errs toward refusing a read rather than admitting a write.
fn is_write(request: &Request) -> bool {
    !matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS")
}

pub async fn maintenance_guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if always_served(request.uri().path()) {
        return next.run(request).await;
    }

    let mut conn = match state.identity_pool.acquire().await {
        Ok(conn) => conn,
        // The database being unreachable is not a reason to refuse: the handler is about to fail on its own with an error that says so, which is more useful than a maintenance notice nobody switched on.
        Err(_) => return next.run(request).await,
    };

    let current = match maintenance::get(&mut *conn).await {
        Ok(current) => current,
        Err(err) => return ApiError::from(err).into_response(),
    };
    drop(conn);

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

#[cfg(test)]
#[path = "../../../tests/http/middleware/maintenance.rs"]
mod tests;
