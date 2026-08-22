//! A per-key fixed-window rate limiter, applied to whichever routes are reachable without a
//! bearer token: `/auth/signup` and `/auth/login`, the ones exposed to unauthenticated
//! credential/invite-token brute-forcing.
//!
//! Keyed by client IP; falls back to a single shared bucket when no `ConnectInfo` is present on
//! the request. Loco's own boot path (`app.into_make_service_with_connect_info::<SocketAddr>()`)
//! always populates it, so this only matters for a request driven directly through the router
//! without going through a real socket (e.g. some test harnesses).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Paths this guard applies to. Anything else passes through untouched.
fn is_guarded(path: &str) -> bool {
    matches!(path, "/auth/signup" | "/auth/login")
}

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

    /// `YORISHIRO_AUTH_RATE_LIMIT_MAX` (default 10) requests per
    /// `YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS` (default 60) seconds, per client IP.
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

    /// Returns `true` if this call is within the limit, `false` if `key` has exhausted its quota
    /// for the current window. The window resets lazily on the first call after it elapses,
    /// rather than on a background timer.
    pub fn allow(&self, key: &str) -> bool {
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();

        // Lazy GC: evict expired entries every 128 calls to bound memory growth. Without this,
        // an attacker rotating source IPs would grow the map without limit (the rate limiter
        // itself becoming a DoS vector).
        if buckets.len() > 128 {
            let window = self.window;
            buckets.retain(|_, (start, _)| now.duration_since(*start) < window);
        }

        let entry = buckets.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        let was_empty = entry.1 == 0;
        entry.1 = entry.1.saturating_add(1);
        entry.1 <= self.max_requests || was_empty
    }
}

pub async fn enforce(
    State(limiter): State<Arc<RateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    if !is_guarded(req.uri().path()) {
        return next.run(req).await;
    }

    let key = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !limiter.allow(&key) {
        // Logged so an operator can see abuse (credential/invite-token brute-forcing) that the
        // access log would otherwise show only as anonymous 429s.
        tracing::warn!(client = %key, path = %req.uri().path(), "auth rate limit exceeded");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    next.run(req).await
}
