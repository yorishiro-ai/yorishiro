use std::time::Duration;

use crate::http::middleware::rate_limit::RateLimiter;
use axum::http::StatusCode;

#[test]
fn allows_requests_within_the_limit() {
    let limiter = RateLimiter::new(3, Duration::from_secs(60));
    assert!(limiter.allow("1.2.3.4"));
    assert!(limiter.allow("1.2.3.4"));
    assert!(limiter.allow("1.2.3.4"));
}

#[test]
fn rejects_requests_past_the_limit() {
    let limiter = RateLimiter::new(2, Duration::from_secs(60));
    assert!(limiter.allow("1.2.3.4"));
    assert!(limiter.allow("1.2.3.4"));
    assert!(!limiter.allow("1.2.3.4"));
}

#[test]
fn tracks_separate_keys_independently() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60));
    assert!(limiter.allow("1.2.3.4"));
    assert!(limiter.allow("5.6.7.8"));
    assert!(!limiter.allow("1.2.3.4"));
}

#[test]
fn resets_after_the_window_elapses() {
    let limiter = RateLimiter::new(1, Duration::from_millis(50));
    assert!(limiter.allow("1.2.3.4"));
    assert!(!limiter.allow("1.2.3.4"));
    std::thread::sleep(Duration::from_millis(60));
    assert!(limiter.allow("1.2.3.4"));
}

#[tracing_test::traced_test]
#[tokio::test]
async fn logs_a_warning_when_the_rate_limit_is_exceeded() {
    use crate::http::middleware::rate_limit::enforce;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    let limiter = std::sync::Arc::new(RateLimiter::new(1, Duration::from_secs(60)));
    let app = Router::new()
        .route("/probe", get(|| async { StatusCode::OK }))
        .layer(axum::middleware::from_fn_with_state(limiter, enforce));

    // First request consumes the only allowed slot for this test's shared bucket
    // (no ConnectInfo is populated by `oneshot`, so every call falls into "unknown").
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!logs_contain("auth rate limit exceeded"));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(logs_contain("auth rate limit exceeded"));
}

/// `apply_rate_limit_layer` exists so a downstream crate merging its own routes against this
/// crate's router (which loses layers not applied directly to those routes -- see
/// `build_app`'s doc comment) can protect its own unauthenticated routes with the same
/// mechanism this crate uses for `/auth/signup`/`/auth/login`/`/setup`/`/setup/status`. This
/// asserts the helper actually enforces the limit once applied, the way a downstream crate
/// would use it -- not just that `enforce` does when wired in by hand (already covered above).
#[tokio::test]
async fn apply_rate_limit_layer_enforces_the_limit_on_a_merged_router() {
    use crate::http::middleware::rate_limit::apply_rate_limit_layer;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    let limiter = std::sync::Arc::new(RateLimiter::new(1, Duration::from_secs(60)));
    let downstream_routes = apply_rate_limit_layer(
        Router::new().route("/auth/oauth/callback", get(|| async { StatusCode::OK })),
        limiter,
    );
    // Simulates a downstream crate merging its own (now rate-limited) routes with this
    // crate's router -- the merge itself must not disturb the layer already applied above.
    let app = downstream_routes.merge(Router::new().route("/up", get(|| async { StatusCode::OK })));

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/callback")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/callback")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// The whole point of taking `Arc<RateLimiter>` as a parameter (rather than having
/// `apply_rate_limit_layer` build its own from `RateLimiter::from_env()`) is that a downstream
/// crate can pass this crate's own limiter and share one quota across both sides' routes,
/// instead of each side getting its own independent bucket -- e.g. a caller shouldn't get 10
/// free requests against `/auth/login` and *another* 10 against a downstream `/auth/oauth/
/// callback` before either side's limiter notices. This asserts that sharing the `Arc` really
/// does produce one shared quota, not two independent ones.
#[tokio::test]
async fn apply_rate_limit_layer_shares_one_quota_when_given_the_same_arc() {
    use crate::http::middleware::rate_limit::apply_rate_limit_layer;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    let limiter = std::sync::Arc::new(RateLimiter::new(2, Duration::from_secs(60)));
    let downstream_routes = apply_rate_limit_layer(
        Router::new().route("/auth/oauth/callback", get(|| async { StatusCode::OK })),
        limiter.clone(),
    );
    let community_routes = apply_rate_limit_layer(
        Router::new().route("/auth/login", get(|| async { StatusCode::OK })),
        limiter,
    );
    let app = downstream_routes.merge(community_routes);

    // Both requests land in the same "unknown" bucket (no ConnectInfo under `oneshot`) despite
    // hitting different routes -- if the quota were independent per side, both would still be
    // OK here since each side would have its own untouched allowance of 2.
    for uri in ["/auth/oauth/callback", "/auth/login"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // A third request, on either route, exhausts the shared quota of 2.
    let third = app
        .oneshot(
            Request::builder()
                .uri("/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
}
