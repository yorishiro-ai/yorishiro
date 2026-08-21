use crate::http::controllers::dashboard::tenant_overview;
use crate::state::HostedState;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use tracing_test::traced_test;
use uuid::Uuid;
use yorishiro_core::db::TenantDb;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use crate::tests::test_helpers;
use test_helpers::hosted_state;

fn router(state: HostedState) -> Router {
    Router::new()
        .route(
            "/hosted/tenant/overview",
            axum::routing::get(tenant_overview),
        )
        .with_state(state)
}

async fn issue_key(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Uuid,
    user_id: Option<Uuid>,
) -> String {
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    create_api_key(&mut *conn, workspace_id, ApiKeyScope::Schema, user_id)
        .await
        .unwrap()
        .plaintext
}

#[sqlx::test(migrations = "../../../migrations")]
async fn owner_can_read_the_tenant_overview(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "prod", None, None, None)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = tenancy::create_user(&mut *conn, "owner@acme.test", "password123", None)
        .await
        .unwrap();
    tenancy::add_member(&mut *conn, tenant.id, user.id, MembershipRole::Owner)
        .await
        .unwrap();
    let key = issue_key(&pool, tenant.id, workspace.id, Some(user.id)).await;

    let app = router(hosted_state(pool));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/hosted/tenant/overview")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["tenant_id"], tenant.id.to_string());
    assert_eq!(json["usage"]["workspace_count"], 1);
    assert_eq!(json["members"][0]["email"], "owner@acme.test");
}

#[sqlx::test(migrations = "../../../migrations")]
async fn member_role_is_forbidden_from_the_dashboard(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "prod", None, None, None)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = tenancy::create_user(&mut *conn, "member@acme.test", "password123", None)
        .await
        .unwrap();
    tenancy::add_member(&mut *conn, tenant.id, user.id, MembershipRole::Member)
        .await
        .unwrap();
    let key = issue_key(&pool, tenant.id, workspace.id, Some(user.id)).await;

    let app = router(hosted_state(pool));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/hosted/tenant/overview")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../../migrations")]
async fn missing_bearer_token_is_unauthorized(pool: PgPool) {
    let app = router(hosted_state(pool));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/hosted/tenant/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[traced_test]
#[sqlx::test(migrations = "../../../migrations")]
async fn logs_a_warning_when_the_bearer_token_is_missing(pool: PgPool) {
    let app = router(hosted_state(pool));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/hosted/tenant/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(logs_contain(
        "hosted dashboard request rejected during authentication"
    ));
}

#[traced_test]
#[sqlx::test(migrations = "../../../migrations")]
async fn logs_a_warning_when_the_bearer_key_does_not_authenticate(pool: PgPool) {
    let app = router(hosted_state(pool));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/hosted/tenant/overview")
                .header("authorization", "Bearer ysr_not_a_real_key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(logs_contain(
        "hosted dashboard request rejected during authentication"
    ));
    // The presented key itself must never reach the logs.
    assert!(!logs_contain("ysr_not_a_real_key"));
}
