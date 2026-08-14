//! The licence gate on the marketplace routes. The marketplace's own behaviour -- publishing,
//! forking, reviews, visibility -- is covered at the service level in `tests/services/marketplace.rs`;
//! what is asserted here is only that the gate opens and closes, which needs the HTTP layer.

use crate::http::controllers::marketplace::list_marketplace;
use crate::state::HostedState;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use yorishiro_core::db::TenantDb;
use yorishiro_core::repositories::tenancy;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use crate::tests::test_helpers;
use test_helpers::{hosted_state, unlicensed_hosted_state};

fn router(state: HostedState) -> Router {
    Router::new()
        .route("/api/marketplace", axum::routing::get(list_marketplace))
        .with_state(state)
}

async fn issue_key(pool: &PgPool, tenant_id: Uuid, workspace_id: Uuid) -> String {
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    create_api_key(&mut conn, workspace_id, ApiKeyScope::Schema, None)
        .await
        .unwrap()
        .plaintext
}

async fn seed_key(pool: &PgPool) -> String {
    let tenant = tenancy::create_tenant(pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(pool, tenant.id, "prod", None, None, None)
        .await
        .unwrap();
    issue_key(pool, tenant.id, workspace.id).await
}

async fn get_marketplace(state: HostedState, key: &str) -> StatusCode {
    router(state)
        .oneshot(
            Request::builder()
                .uri("/api/marketplace")
                .header("Authorization", format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn an_active_licence_opens_the_marketplace(pool: PgPool) {
    let key = seed_key(&pool).await;

    let status = get_marketplace(hosted_state(pool), &key).await;

    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn without_a_licence_the_marketplace_is_not_served(pool: PgPool) {
    let key = seed_key(&pool).await;

    let status = get_marketplace(unlicensed_hosted_state(pool), &key).await;

    // 404, not 402 or 403: the deployment genuinely does not serve this. The same answer a
    // deployment gets for the setup wizard it has disabled.
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The gate runs before authentication, so an unlicensed deployment cannot be probed for which
/// paid features it would have had. A 401 here would confirm the route exists.
#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key(pool: PgPool) {
    let status = get_marketplace(unlicensed_hosted_state(pool), "ysr_not_a_real_key").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A licensed deployment still authenticates. Without this, "gated" and "open to anyone" would
/// look the same in the test above.
#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn a_licence_does_not_replace_authentication(pool: PgPool) {
    let status = get_marketplace(hosted_state(pool), "ysr_not_a_real_key").await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A key that lapsed while the process was running closes the gate on the next request, without
/// a restart -- the state holds claims, not a boolean captured at boot.
#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]
async fn a_licence_that_expired_mid_run_closes_the_gate(pool: PgPool) {
    use crate::services::licence::{LicenceClaims, LicenceState};

    let key = seed_key(&pool).await;
    let expired = LicenceState::licensed(LicenceClaims {
        sub: "acme-corp".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() - 60 * 60,
    });
    let state = HostedState {
        licence: expired,
        ..hosted_state(pool)
    };

    assert_eq!(get_marketplace(state, &key).await, StatusCode::NOT_FOUND);
}
