use super::*;

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
