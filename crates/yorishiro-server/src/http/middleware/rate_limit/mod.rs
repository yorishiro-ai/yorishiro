use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// A per-key fixed-window rate limiter, applied via [`apply_rate_limit_layer`] to whichever
/// routes a caller considers reachable without a bearer token (in this crate, `/auth/signup`,
/// `/auth/login`, `/setup`, `/setup/status`) -- the ones exposed to unauthenticated
/// credential/invite-token brute-forcing. A downstream crate adding its own unauthenticated
/// routes (e.g. an OAuth callback) needs the same protection; see `apply_rate_limit_layer`'s
/// doc comment. Keyed by client IP; falls back to a single shared bucket when no `ConnectInfo`
/// is present on the request (e.g. tests driven through `Router::oneshot`, which never
/// populates it).
pub struct RateLimiter {
    max_requests: u32,
    window: Duration,
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` (default 100000) tokens per minute, per workspace.
    ///
    /// Keyed by workspace rather than by IP: a search is authenticated, so the workspace is
    /// known and is the thing whose consumption matters. The default is high enough that
    /// ordinary use never reaches it — it is there to bound a runaway agent, not to ration.
    pub fn search_tokens_from_env() -> Self {
        let max_tokens = std::env::var("YORISHIRO_SEARCH_TOKENS_PER_MINUTE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000);
        Self::new(max_tokens, Duration::from_secs(60))
    }

    /// `YORISHIRO_AUTH_RATE_LIMIT_MAX` (default 10) requests per `YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS`
    /// (default 60) seconds, per client IP.
    pub fn from_env() -> Self {
        let max_requests = std::env::var("YORISHIRO_AUTH_RATE_LIMIT_MAX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let window_secs = std::env::var("YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        Self::new(max_requests, Duration::from_secs(window_secs))
    }

    /// Returns `true` if this call is within the limit, `false` if `key` has exhausted its
    /// quota for the current window. The window resets lazily on the first call after it
    /// elapses, rather than on a background timer.
    ///
    /// `pub` (rather than private) solely so the crate-root `tests/` integration tests --
    /// which only see this crate's public API -- can exercise the bucket logic directly
    /// instead of only through `enforce`.
    pub fn allow(&self, key: &str) -> bool {
        self.allow_cost(key, 1)
    }

    /// As [`Self::allow`], charging `cost` against the window instead of one.
    ///
    /// A quota counted in requests treats a one-word query and a paragraph alike, though the
    /// second costs the embedding model proportionally more. Charging the token count instead
    /// bounds the work rather than the call count.
    ///
    /// A single request larger than the whole window is still admitted, once: rejecting it
    /// would make that query permanently impossible rather than merely expensive, and the
    /// bucket is left exhausted so the next one waits.
    pub fn allow_cost(&self, key: &str, cost: u32) -> bool {
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();

        // Lazy GC: evict expired entries every 128 calls to bound memory growth.
        // Without this, an attacker rotating source IPs would grow the map without
        // limit (the rate limiter itself becoming a DoS vector).
        if buckets.len() > 128 {
            let window = self.window;
            buckets.retain(|_, (start, _)| now.duration_since(*start) < window);
        }

        let entry = buckets.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        let was_empty = entry.1 == 0;
        entry.1 = entry.1.saturating_add(cost);
        entry.1 <= self.max_requests || was_empty
    }
}

/// Applies [`enforce`] to `router`, keyed by `limiter`. Factored out so a process embedding
/// this crate's routes alongside its own (e.g. a downstream crate adding an OAuth
/// login/callback pair, which is exactly as reachable without a bearer token as
/// `/auth/login`) can rate-limit its own unauthenticated routes the same way this crate does
/// its own -- `axum::Router::merge` doesn't propagate a `.layer()` from either side to the
/// other, so each sub-router must carry its own copy.
///
/// Pass the *same* `Arc<RateLimiter>` used for this crate's own auth routes (for a downstream
/// crate embedding [`crate::build_app`], reuse the limiter that produced its rate-limited
/// routes -- see that function's doc comment) to share one quota across both; pass a fresh
/// `Arc::new(RateLimiter::from_env())` instead for an independent quota.
pub fn apply_rate_limit_layer<S>(router: Router<S>, limiter: Arc<RateLimiter>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn_with_state(limiter, enforce))
}

pub async fn enforce(
    State(limiter): State<std::sync::Arc<RateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    let key = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !limiter.allow(&key) {
        // Logged so an operator can see abuse (credential/invite-token brute-forcing) that
        // the access log would otherwise show only as anonymous 429s.
        tracing::warn!(client = %key, path = %req.uri().path(), "auth rate limit exceeded");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    next.run(req).await
}

#[cfg(test)]
#[path = "../../../../tests/http/middleware/rate_limit/mod.rs"]
mod tests;
