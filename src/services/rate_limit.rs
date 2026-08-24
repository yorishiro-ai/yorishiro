//! A per-key fixed-window rate limiter, applied to whichever routes are reachable without a bearer token: `/auth/signup` and `/auth/login`, plus `ee/`'s `/auth/oauth/authorize` and `/auth/oauth/callback` when that crate is composed in.
//! Base has no compile-time dependency on `ee/`, so the `ee/` paths are named as string literals rather than imported; they're absent from the router entirely when `ee/` isn't linked in, so the match is simply never reached.
//! `/auth/oauth/status` is deliberately excluded, since it carries no secret and the login page polls it on every load.
//!
//! Keyed by client IP; falls back to a single shared bucket when no `ConnectInfo` is present on the request (Loco's boot path always populates it, so this only matters for a request driven directly through the router, e.g. some test harnesses).

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
    matches!(
        path,
        "/auth/signup" | "/auth/login" | "/auth/oauth/authorize" | "/auth/oauth/callback"
    )
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

    /// `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` (default 100000) tokens per minute, per workspace.
    ///
    /// Keyed by workspace rather than by IP: a search is authenticated, so the workspace is known and is the thing whose consumption matters.
    /// The default is high enough that ordinary use never reaches it: it is there to bound a runaway agent, not to ration.
    pub fn search_tokens_from_env() -> Self {
        let max_tokens = std::env::var("YORISHIRO_SEARCH_TOKENS_PER_MINUTE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000);
        Self::new(max_tokens, Duration::from_secs(60))
    }

    /// Returns `true` if this call is within the limit, `false` if `key` has exhausted its quota for the current window.
    /// The window resets lazily on the first call after it elapses, rather than on a background timer.
    pub fn allow(&self, key: &str) -> bool {
        self.allow_cost(key, 1)
    }

    /// As [`Self::allow`], charging `cost` against the window instead of one.
    ///
    /// A quota counted in requests treats a one-word query and a paragraph alike, though the second costs the embedding model proportionally more.
    /// Charging the token count instead bounds the work rather than the call count.
    ///
    /// A single request larger than the whole window is still admitted, once: rejecting it would make that query permanently impossible rather than merely expensive, and the bucket is left exhausted so the next one waits.
    pub fn allow_cost(&self, key: &str, cost: u32) -> bool {
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();

        // Lazy GC: evict expired entries every 128 calls to bound memory growth.
        // Without this, an attacker rotating source IPs would grow the map without limit (the rate limiter itself becoming a DoS vector).
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

/// Charges a search query against its workspace's token budget, refusing when the budget is spent.
///
/// Charged before embedding, since embedding is the work the budget protects, and counting is cheap (a query is short), which is why search is metered in tokens while writes stay on request counts.
/// A free function taking both `ctx` values rather than a method on either, so a check written for only one caller can't leave the other able to spend the budget it's meant to protect: both the REST and MCP search handlers call this same function.
pub fn charge_search_tokens(
    limiter: &RateLimiter,
    provider: &dyn crate::services::embedding::EmbeddingProvider,
    workspace_id: uuid::Uuid,
    query_text: &str,
) -> Result<(), crate::error::YorishiroError> {
    let tokens = provider.count_tokens(query_text);
    if limiter.allow_cost(&workspace_id.to_string(), tokens) {
        return Ok(());
    }

    tracing::warn!(%workspace_id, tokens, "search token budget exhausted");
    Err(crate::error::YorishiroError::ValidationFailed {
        message: "this workspace has spent its search token budget for the minute".to_string(),
        details: vec![],
        hint: "retry shortly, or raise YORISHIRO_SEARCH_TOKENS_PER_MINUTE".to_string(),
    })
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;
    use uuid::Uuid;

    use super::{RateLimiter, charge_search_tokens, is_guarded};
    use crate::error::YorishiroError;
    use crate::services::embedding::EmbeddingProvider;

    #[test]
    fn guards_base_and_ee_credential_issuing_routes() {
        assert!(is_guarded("/auth/signup"));
        assert!(is_guarded("/auth/login"));
        assert!(is_guarded("/auth/oauth/authorize"));
        assert!(is_guarded("/auth/oauth/callback"));
    }

    #[test]
    fn does_not_guard_oauth_status_or_unrelated_paths() {
        assert!(!is_guarded("/auth/oauth/status"));
        assert!(!is_guarded("/api/workspaces"));
        assert!(!is_guarded("/setup"));
    }

    /// Counts tokens as the byte length exactly, so a test can pick a query whose cost is known up front rather than depending on the default estimate's rounding.
    struct FixedCostProvider;

    #[async_trait]
    impl EmbeddingProvider for FixedCostProvider {
        fn dimensions(&self) -> usize {
            1
        }
        fn count_tokens(&self, text: &str) -> u32 {
            text.len() as u32
        }
        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            unimplemented!("not exercised by these tests")
        }
    }

    #[test]
    fn charge_search_tokens_exhausts_the_budget_and_then_rejects() {
        let limiter = RateLimiter::new(10, Duration::from_secs(60));
        let provider = FixedCostProvider;
        let workspace = Uuid::new_v4();

        // "0123456789" costs 10 tokens: exactly the budget, so this call is admitted.
        charge_search_tokens(&limiter, &provider, workspace, "0123456789").unwrap();
        // The budget is now spent; the same workspace's next call is rejected.
        assert!(charge_search_tokens(&limiter, &provider, workspace, "x").is_err());
    }

    #[test]
    fn charge_search_tokens_is_keyed_per_workspace() {
        let limiter = RateLimiter::new(10, Duration::from_secs(60));
        let provider = FixedCostProvider;
        let workspace = Uuid::new_v4();
        let other = Uuid::new_v4();

        charge_search_tokens(&limiter, &provider, workspace, "0123456789").unwrap();
        assert!(charge_search_tokens(&limiter, &provider, workspace, "x").is_err());
        // A different workspace has its own, untouched budget.
        charge_search_tokens(&limiter, &provider, other, "0123456789").unwrap();
    }
}
