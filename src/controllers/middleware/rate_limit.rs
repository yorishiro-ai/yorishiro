//! A per-key fixed-window rate limiter, applied to the credential routes reachable without a bearer token.
//! Which paths those are is not decided here: [`crate::app::RouteMounts`] collects them as route groups are mounted, so a route group added by another edition is guarded by naming its path when it is mounted, and the limiter never learns who added it.
//!
//! Keyed by client IP; falls back to a single shared bucket when no `ConnectInfo` is present on the request (Loco's boot path always populates it, so this only matters for a request driven directly through the router, e.g. some test harnesses).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ONE_MINUTE: Duration = Duration::from_secs(60);
const GC_BUCKET_THRESHOLD: usize = 128;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub struct RateLimiter {
    max_requests: u32,
    window: Duration,
    guarded_paths: Vec<&'static str>,
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            guarded_paths: Vec::new(),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Makes [`enforce`] apply this limiter to `paths`.
    /// Any other path passes through untouched.
    #[must_use]
    pub(crate) fn guarding(mut self, paths: &[&'static str]) -> Self {
        self.guarded_paths = paths.to_vec();
        self
    }

    /// `YORISHIRO_AUTH_RATE_LIMIT_MAX` (default 10) requests per
    /// `YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS` (default 60) seconds, per client IP.
    pub(crate) fn auth(config: &crate::data::settings::Settings) -> Self {
        Self::new(
            config.rate_limit.auth_max_requests,
            Duration::from_secs(config.rate_limit.auth_window_seconds),
        )
    }

    /// `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` (default 100000) tokens per minute, per workspace.
    ///
    /// Keyed by workspace rather than by IP: a search is authenticated, so the workspace is known and is the thing whose consumption matters.
    /// The default is high enough that ordinary use never reaches it: it is there to bound a runaway agent, not to ration.
    pub(crate) fn search(config: &crate::data::settings::Settings) -> Self {
        Self::new(config.rate_limit.search_tokens_per_minute, ONE_MINUTE)
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
    pub(crate) fn allow_cost(&self, key: &str, cost: u32) -> bool {
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();

        // Lazy GC: evict expired entries every 128 calls to bound memory growth.
        // Without this, an attacker rotating source IPs would grow the map without limit (the rate limiter itself becoming a DoS vector).
        if buckets.len() > GC_BUCKET_THRESHOLD {
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
/// The server holds no tokenizer, so a query costs one token per four bytes, rounded up: the estimate every provider without its own tokenizer already uses, and an overestimate for non-English text, which suits a quota.
/// A free function taking both `ctx` values rather than a method on either, so a check written for only one caller can't leave the other able to spend the budget it's meant to protect: both the REST and MCP search handlers call this same function.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn charge_search_tokens(
    limiter: &RateLimiter,
    workspace_id: uuid::Uuid,
    query_text: &str,
) -> Result<(), crate::error::YorishiroError> {
    let tokens = u32::try_from(query_text.len().div_ceil(4)).unwrap_or(u32::MAX);
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
    if !limiter.guarded_paths.contains(&req.uri().path()) {
        return next.run(req).await;
    }

    let key = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !limiter.allow(&key) {
        // Logged so an operator can see abuse (credential/invite-token brute-forcing) that the access log would otherwise show only as anonymous 429s.
        tracing::warn!(client = %key, path = %req.uri().path(), "auth rate limit exceeded");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    next.run(req).await
}
