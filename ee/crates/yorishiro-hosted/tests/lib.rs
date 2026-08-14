//! Exercises how `yorishiro_server`'s `main` actually wires `apply_rate_limit_layer`/
//! `apply_body_limit_layer` (see that file) -- `oauth_login_router()`'s two routes share the
//! community server's own `/auth/login`/`/auth/signup`/`/setup*` rate-limit quota, while
//! `router()`'s routes (the dashboard, the Stripe webhook, and `/auth/oauth/status`) are
//! deliberately outside it; every route in both sub-routers still gets the 2 MiB body cap.
//! These tests build the same layered shape `main` does, rather than going through
//! `crate::router()` directly, since the routing crate itself is not allowed to
//! depend on `yorishiro-server` (see CLAUDE.md) and so cannot apply either layer itself -- only
//! the binary can.

use std::time::Duration;

use crate::state::HostedState;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_server::apply_body_limit_layer;
use yorishiro_server::http::middleware::rate_limit::{RateLimiter, apply_rate_limit_layer};

#[path = "test_helpers.rs"]
pub mod test_helpers;
use test_helpers::hosted_state;

/// Mirrors `yorishiro_server::run`'s `oauth_login_router` construction, with an
/// injectable `RateLimiter` so tests can use a tiny limit instead of `from_env`'s default of 10
/// requests/60s (which would make a "goes over the limit" test either slow or require sending
/// 11 requests -- both avoidable).
fn rate_limited_oauth_router(state: HostedState, limiter: std::sync::Arc<RateLimiter>) -> Router {
    apply_rate_limit_layer(crate::oauth_login_router().with_state(state), limiter)
}

/// The rest of `crate::router()`, isolating the one property these tests care about:
/// no rate limiter attached, matching `main`. `main` itself also wraps this sub-router in
/// `apply_body_limit_layer`/`apply_observability_layers` -- see `body_limited_router` below for
/// the body-limit half.
fn unlimited_router(state: HostedState) -> Router {
    crate::router().with_state(state)
}

/// Mirrors `yorishiro_server::run`'s `hosted_router` construction: `router()`'s routes
/// with the same 2 MiB body cap `build_app`'s own routes get, but no rate limiter -- see
/// `unlimited_router` for that half.
fn body_limited_router(state: HostedState) -> Router {
    apply_body_limit_layer(crate::router().with_state(state))
}

/// `GET /auth/oauth/authorize` is one of the two routes `main` rate-limits. A request past the
/// limit must get `429`, the same as base's own `/auth/login` under `apply_rate_limit_layer` --
/// see `yorishiro-server`'s `http_middleware_rate_limit.rs` tests for the upstream equivalent
/// this mirrors.
#[sqlx::test(migrations = "../../../migrations")]
async fn authorize_429s_once_the_rate_limit_is_exceeded(pool: PgPool) {
    let limiter = std::sync::Arc::new(RateLimiter::new(1, Duration::from_secs(60)));
    let app = rate_limited_oauth_router(hosted_state(pool), limiter);

    // First request consumes the only allowed slot for this test's shared bucket (no
    // `ConnectInfo` is populated by `oneshot`, so every call here falls into the same "unknown"
    // key -- see `RateLimiter`'s doc comment). OAuth isn't configured, so the route itself 404s,
    // but that happens *after* the rate-limit middleware runs -- what's asserted here is that
    // the first call reaches the handler at all (not a `429`), not what the handler then does.
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/authorize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/authorize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// `GET /auth/oauth/callback` is the other rate-limited route -- it's the one that can actually
/// end up issuing an API key from caller-supplied input (an authorization code), so it needs the
/// same protection `authorize` gets, not a separate, looser one.
#[sqlx::test(migrations = "../../../migrations")]
async fn callback_429s_once_the_rate_limit_is_exceeded(pool: PgPool) {
    let limiter = std::sync::Arc::new(RateLimiter::new(1, Duration::from_secs(60)));
    let app = rate_limited_oauth_router(hosted_state(pool), limiter);

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/callback?code=abc&state=xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/callback?code=abc&state=xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// `authorize` and `callback` share one `Arc<RateLimiter>` in `main` (so an attacker who
/// exhausts one can't get a fresh quota from the other) -- asserted the same way base's own
/// `apply_rate_limit_layer_shares_one_quota_when_given_the_same_arc` proves it for its own
/// routes.
#[sqlx::test(migrations = "../../../migrations")]
async fn authorize_and_callback_share_one_quota(pool: PgPool) {
    let limiter = std::sync::Arc::new(RateLimiter::new(2, Duration::from_secs(60)));
    let app = rate_limited_oauth_router(hosted_state(pool), limiter);

    for uri in [
        "/auth/oauth/authorize",
        "/auth/oauth/callback?code=abc&state=xyz",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // A third request, on either route, exhausts the shared quota of 2.
    let third = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/authorize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// `/auth/oauth/status` must never be rate-limited -- the Web UI's login page calls it on every
/// load, and it returns no secret, so there's nothing to protect by limiting it.
#[sqlx::test(migrations = "../../../migrations")]
async fn status_is_never_rate_limited(pool: PgPool) {
    let app = unlimited_router(hosted_state(pool));

    for _ in 0..25 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/oauth/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

/// `/hosted/stripe/webhook` must never be rate-limited: it's a signature-verified webhook
/// Stripe itself calls, not an attacker-reachable unauthenticated route, and dropping a
/// legitimate billing event on a `429` is worse than not rate-limiting it. Posting with no
/// signature header gets a `401` from the handler's own verification, not a `429` from a rate
/// limiter that was never applied to this route -- the repeated 401s themselves are the
/// assertion that nothing here is counting or capping requests.
#[sqlx::test(migrations = "../../../migrations")]
async fn stripe_webhook_is_never_rate_limited(pool: PgPool) {
    let app = unlimited_router(hosted_state(pool));

    for _ in 0..25 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hosted/stripe/webhook")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}

/// Asserts the 2 MiB cap holds on `/hosted/stripe/webhook` -- not, on its own, that
/// `apply_body_limit_layer` is what produces it: axum's `Bytes`/`Json`/`String` extractors fall
/// back to the same 2 MiB default with no layer applied at all (see `docs/api.md`'s "Request
/// bodies" paragraph), so a 3 MiB body was already rejected before `main` started applying this
/// layer explicitly, and this test alone can't tell the two apart. The layer is still applied
/// (matching base's documented `build_app`-composition pattern) as hardening beyond what this
/// test covers: the extractor default only protects handlers that actually consume the body via
/// `Bytes`/`Json`/`String`, so a future handler taking a raw `Request` or a streaming body would
/// have no cap without it.
#[sqlx::test(migrations = "../../../migrations")]
async fn stripe_webhook_rejects_a_body_over_the_2mib_cap(pool: PgPool) {
    let app = body_limited_router(hosted_state(pool));

    let oversized = vec![0u8; 3 * 1024 * 1024];
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/hosted/stripe/webhook")
                .body(Body::from(oversized))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// Never called: the maintenance guard runs before any handler that would embed, and none of
/// the routes in this crate embed at all. Present only because `AppState::new` requires a
/// provider.
struct UnusedEmbeddingProvider;

#[async_trait::async_trait]
impl yorishiro_core::services::embedding::EmbeddingProvider for UnusedEmbeddingProvider {
    fn dimensions(&self) -> usize {
        1024
    }

    async fn embed_batch(
        &self,
        _texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, yorishiro_core::error::YorishiroError> {
        unreachable!("no route in this crate embeds")
    }
}

/// Mirrors how `main` wraps each of this crate's sub-routers in the maintenance guard, using
/// the same `AppState` the community edition's own routes are guarded with.
fn guarded(router: Router, pool: PgPool) -> Router {
    let app_state = yorishiro_server::AppState::new(
        yorishiro_core::db::TenantDb::new(pool.clone()),
        pool,
        std::sync::Arc::new(UnusedEmbeddingProvider),
    );
    router.layer(axum::middleware::from_fn_with_state(
        app_state,
        yorishiro_server::http::middleware::maintenance::maintenance_guard,
    ))
}

/// Pausing the deployment has to stop this crate's writes too.
///
/// The guard is applied inside `build_app`, and neither `merge` nor `fallback_service`
/// propagates a layer any more than `.layer()` does -- so a router composed the way `main`
/// composes this one gets no guard unless `main` applies it separately. Without that, an
/// operator who switched the deployment to read-only would still have `/hosted/stripe/webhook`
/// accepting billing events: the community edition's `/api/*` would refuse while this crate's
/// routes kept writing, which is worse than either refusing everything or refusing nothing,
/// because the two halves of one deployment would disagree about whether it is paused.
#[sqlx::test(migrations = "../../../migrations")]
async fn read_only_mode_refuses_this_crates_writes(pool: PgPool) {
    yorishiro_core::repositories::maintenance::set(
        &pool,
        yorishiro_core::repositories::maintenance::MaintenanceMode::ReadOnly,
        30,
        None,
    )
    .await
    .unwrap();

    let app = guarded(unlimited_router(hosted_state(pool.clone())), pool);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/hosted/stripe/webhook")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::LOCKED,
        "a write to this crate's routes must be refused while the deployment is read-only"
    );
}

/// The mirror image: read-only stops writes, not reads. `/auth/oauth/status` is what the login
/// page polls, so refusing it would make a paused deployment look broken rather than paused.
#[sqlx::test(migrations = "../../../migrations")]
async fn read_only_mode_still_serves_this_crates_reads(pool: PgPool) {
    yorishiro_core::repositories::maintenance::set(
        &pool,
        yorishiro_core::repositories::maintenance::MaintenanceMode::ReadOnly,
        30,
        None,
    )
    .await
    .unwrap();

    let app = guarded(unlimited_router(hosted_state(pool.clone())), pool);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// A full lock refuses reads as well, including the OAuth login pair -- letting someone start a
/// login flow against a fully locked deployment would hand them an API key for a system that is
/// about to refuse every call they make with it.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_full_lock_refuses_the_oauth_login_routes(pool: PgPool) {
    yorishiro_core::repositories::maintenance::set(
        &pool,
        yorishiro_core::repositories::maintenance::MaintenanceMode::FullLock,
        30,
        None,
    )
    .await
    .unwrap();

    let limiter = std::sync::Arc::new(RateLimiter::new(10, Duration::from_secs(60)));
    let app = guarded(
        rate_limited_oauth_router(hosted_state(pool.clone()), limiter),
        pool,
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/authorize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
