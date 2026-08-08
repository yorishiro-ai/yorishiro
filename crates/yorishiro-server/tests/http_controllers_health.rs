use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::embedding::EmbeddingProvider;
use yorishiro_server::{AppState, build_app};

struct UnreachableEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for UnreachableEmbeddingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::Internal(anyhow::anyhow!(
            "embedding provider should not be called in this test"
        )))
    }
}

async fn get_response(pool: PgPool, uri: &str) -> axum::response::Response {
    let state = AppState::new(
        TenantDb::new(pool.clone()),
        pool,
        std::sync::Arc::new(UnreachableEmbeddingProvider),
    );
    let app = build_app(state, None);

    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn health_response(pool: PgPool) -> axum::response::Response {
    get_response(pool, "/health").await
}

/// `/up` must stay healthy even when the database is unreachable, since it's a pure
/// liveness probe — that's the property distinguishing it from `/health`.
#[sqlx::test(migrations = "../../migrations")]
async fn up_returns_ok_even_when_db_is_unreachable(pool: PgPool) {
    pool.close().await;

    let response = get_response(pool, "/up").await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[sqlx::test(migrations = "../../migrations")]
async fn health_returns_ok_when_db_is_reachable(pool: PgPool) {
    let response = health_response(pool).await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

/// `Pool::close()` affects every clone that shares the pool, and subsequent `acquire()`
/// calls immediately return `Error::PoolClosed`. This is the easiest way to reproduce a
/// real DB outage / pool exhaustion, so it's used here to exercise the path where an
/// unreachable database results in a 503.
#[sqlx::test(migrations = "../../migrations")]
async fn health_returns_service_unavailable_when_db_is_unreachable(pool: PgPool) {
    pool.close().await;

    let response = health_response(pool).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "unavailable");
}
