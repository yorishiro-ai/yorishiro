use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::services::embedding::EmbeddingProvider;
use yorishiro_core::YorishiroError;

use super::*;
use crate::state::AppState;

struct UnreachableEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for UnreachableEmbeddingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::Internal(anyhow::anyhow!("unreachable")))
    }
}

fn app(pool: PgPool) -> Router {
    let state = AppState::new(
        TenantDb::new(pool.clone()),
        pool,
        std::sync::Arc::new(UnreachableEmbeddingProvider),
    );
    Router::new()
        .route(
            "/probe",
            get(|AuthContext(_): AuthContext| async { StatusCode::OK }),
        )
        .with_state(state)
}

#[tracing_test::traced_test]
#[sqlx::test(migrations = "../../migrations")]
async fn logs_a_warning_when_the_bearer_header_is_missing(pool: PgPool) {
    let response = app(pool)
        .oneshot(
            Request::builder()
                .uri("/probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(logs_contain("request rejected during authentication"));
    assert!(logs_contain("path"));
    // Never log the presented credential, even though there wasn't one here.
    assert!(!logs_contain("Bearer"));
}

#[tracing_test::traced_test]
#[sqlx::test(migrations = "../../migrations")]
async fn logs_a_warning_when_the_bearer_key_does_not_authenticate(pool: PgPool) {
    let response = app(pool)
        .oneshot(
            Request::builder()
                .uri("/probe")
                .header("authorization", "Bearer ysr_not_a_real_key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(logs_contain("request rejected during authentication"));
    // The presented key itself must never reach the logs.
    assert!(!logs_contain("ysr_not_a_real_key"));
}
