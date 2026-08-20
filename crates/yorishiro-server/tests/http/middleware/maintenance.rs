use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::models::maintenance::{MaintenanceMode, set};

use crate::build_app;
use crate::test_support::*;

async fn status_of(app: &axum::Router, method: &str, path: &str) -> (StatusCode, Option<String>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (response.status(), retry_after)
}

/// Read-only refuses writes and serves reads.
/// The Retry-After header is the part agents act on, so its absence would leave them retrying immediately against a server shedding load.
#[sqlx::test(migrations = "../../migrations")]
async fn read_only_refuses_writes_with_423_and_a_retry_after(pool: PgPool) {
    let app = build_app(test_state(pool.clone()), no_static_fallback());
    set(&pool, MaintenanceMode::ReadOnly, 45, None)
        .await
        .unwrap();

    let (status, retry_after) = status_of(&app, "POST", "/api/entities").await;
    assert_eq!(status, StatusCode::LOCKED);
    assert_eq!(retry_after.as_deref(), Some("45"));

    // A read is not a write, so it gets past the guard: 401 here is the auth layer behind it, which is exactly what "the guard let this through" looks like.
    let (status, _) = status_of(&app, "GET", "/api/entities").await;
    assert_ne!(status, StatusCode::LOCKED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn full_lock_refuses_reads_too_with_503(pool: PgPool) {
    let app = build_app(test_state(pool.clone()), no_static_fallback());
    set(&pool, MaintenanceMode::FullLock, 120, None)
        .await
        .unwrap();

    let (status, retry_after) = status_of(&app, "GET", "/api/entities").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(retry_after.as_deref(), Some("120"));

    let (status, _) = status_of(&app, "POST", "/api/entities").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// The probes answer even under full lock.
/// Refusing them would have an orchestrator restart a server that is deliberately paused.
/// Restarting would not clear the state, which lives in the database, so the loop would never converge.
#[sqlx::test(migrations = "../../migrations")]
async fn liveness_probes_answer_under_full_lock(pool: PgPool) {
    let app = build_app(test_state(pool.clone()), no_static_fallback());
    set(&pool, MaintenanceMode::FullLock, 300, None)
        .await
        .unwrap();

    let (status, _) = status_of(&app, "GET", "/up").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = status_of(&app, "GET", "/health").await;
    assert_eq!(status, StatusCode::OK);
}

/// Turning it off is visible to the next request: the state is read per request rather than cached, so an operator does not wait out a TTL.
#[sqlx::test(migrations = "../../migrations")]
async fn clearing_maintenance_takes_effect_immediately(pool: PgPool) {
    let app = build_app(test_state(pool.clone()), no_static_fallback());

    set(&pool, MaintenanceMode::FullLock, 300, None)
        .await
        .unwrap();
    let (status, _) = status_of(&app, "GET", "/api/entities").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    set(&pool, MaintenanceMode::Off, 300, None).await.unwrap();
    let (status, _) = status_of(&app, "GET", "/api/entities").await;
    assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// A deployment nobody has touched serves normally.
#[sqlx::test(migrations = "../../migrations")]
async fn a_fresh_deployment_serves_normally(pool: PgPool) {
    let app = build_app(test_state(pool), no_static_fallback());
    let (status, _) = status_of(&app, "POST", "/api/entities").await;
    assert_ne!(status, StatusCode::LOCKED);
    assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE);
}
