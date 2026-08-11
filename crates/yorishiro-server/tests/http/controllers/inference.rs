use crate::test_support::*;
use crate::{AppState, build_app};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

async fn app_and_key(pool: &PgPool, scope: ApiKeyScope) -> (axum::Router, String) {
    let (tenant_id, workspace_id) = seed_workspace(pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let created = create_api_key(&mut conn, workspace_id, scope, None)
        .await
        .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );
    (app, created.plaintext)
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<serde_json::Value>,
) -> axum::http::Response<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {key}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    app.clone().oneshot(request).await.unwrap()
}

/// Inference costs the workspace money, so a workspace that configured no key must be told so
/// rather than quietly receiving `default` values -- a caller who asked for inference and got
/// defaults has no way to tell that nothing was inferred.
#[sqlx::test(migrations = "../../migrations")]
async fn inferring_without_a_key_is_refused(pool: PgPool) {
    let (app, key) = app_and_key(&pool, ApiKeyScope::Schema).await;

    let response = request(
        &app,
        "POST",
        "/api/schemas/active/anything/infer-fill",
        &key,
        None,
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a workspace with no LLM credentials must be refused, not fall back to defaults"
    );
}

/// The key goes in and never comes back. `GET` reports the endpoint and model so an operator can
/// confirm what is configured, and nothing that could be replayed.
#[sqlx::test(migrations = "../../migrations")]
async fn the_key_is_never_returned(pool: PgPool) {
    let (app, key) = app_and_key(&pool, ApiKeyScope::Schema).await;

    let stored = request(
        &app,
        "PUT",
        "/api/workspace/llm-key",
        &key,
        Some(serde_json::json!({
            "base_url": "https://api.example.com/v1",
            "model": "gpt-4o-mini",
            "api_key": "sk-secret-value"
        })),
    )
    .await;
    assert_eq!(stored.status(), StatusCode::NO_CONTENT);

    let response = request(&app, "GET", "/api/workspace/llm-key", &key, None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let rendered = String::from_utf8_lossy(&body);

    assert!(rendered.contains("gpt-4o-mini"));
    assert!(
        !rendered.contains("sk-secret-value"),
        "the api key must never be returned: {rendered}"
    );
}

/// Storing credentials changes what the deployment can spend, so it takes the same scope as
/// changing a schema -- a read key must not be able to point the workspace at a provider.
#[sqlx::test(migrations = "../../migrations")]
async fn a_read_key_cannot_configure_credentials(pool: PgPool) {
    let (app, key) = app_and_key(&pool, ApiKeyScope::Read).await;

    let response = request(
        &app,
        "PUT",
        "/api/workspace/llm-key",
        &key,
        Some(serde_json::json!({
            "base_url": "https://api.example.com/v1",
            "model": "m",
            "api_key": "k"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Nothing configured is a 404 rather than an empty body, so a caller can tell "not set up"
/// from "set up with blank values".
#[sqlx::test(migrations = "../../migrations")]
async fn an_unconfigured_workspace_reports_nothing(pool: PgPool) {
    let (app, key) = app_and_key(&pool, ApiKeyScope::Read).await;

    let response = request(&app, "GET", "/api/workspace/llm-key", &key, None).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Confirming applies reviewed values, so it takes schema scope. A write key can change one
/// entity it names; it must not apply a batch of guesses across the workspace.
#[sqlx::test(migrations = "../../migrations")]
async fn a_write_key_cannot_confirm_a_job(pool: PgPool) {
    let (app, key) = app_and_key(&pool, ApiKeyScope::Write).await;

    let response = request(
        &app,
        "POST",
        "/api/migration-jobs/00000000-0000-0000-0000-000000000000/confirm",
        &key,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
