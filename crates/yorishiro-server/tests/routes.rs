use std::time::Duration;

use crate::http::middleware::rate_limit::{RateLimiter, apply_rate_limit_layer};
use crate::{apply_body_limit_layer, apply_observability_layers};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use axum::routing::post;
use tower::ServiceExt;

/// `apply_body_limit_layer` exists so a downstream crate merging its own routes against this
/// crate's router (which loses layers not applied directly to those routes -- see
/// `build_app`'s doc comment) can cap its own routes' request bodies the same way this crate
/// caps its own. This asserts the helper actually enforces the cap once applied and merged,
/// not just that it compiles. The handler must actually consume the body (`Bytes`) -- axum's
/// body-limit layer only rejects an oversized body once something tries to read it.
#[tokio::test]
async fn apply_body_limit_layer_rejects_an_oversized_body_on_a_merged_router() {
    let downstream_routes = apply_body_limit_layer(Router::new().route(
        "/oauth/callback",
        post(|_body: Bytes| async { StatusCode::OK }),
    ));
    let app = downstream_routes
        .merge(Router::new().route("/up", axum::routing::get(|| async { StatusCode::OK })));

    let oversized = vec![0u8; 3 * 1024 * 1024];
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/callback")
                .body(Body::from(oversized))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// `build_app`'s "Merging your own routes in" doc example is `#[doc = "```ignore"]` (it needs
/// a live `AppState`, which a doctest can't construct), so nothing else compile-checks it.
/// This mirrors that exact composition -- `apply_body_limit_layer(apply_observability_layers(
/// apply_rate_limit_layer(r, limiter)))` on a `Router<()>`, merged against another `Router<()>`
/// standing in for `build_app_with_rate_limiter`'s return -- so a signature change to any of
/// the three helpers that would break the documented snippet fails a real test, not just a
/// doc comment nobody re-reads.
#[tokio::test]
async fn build_apps_doc_comment_composition_compiles_and_serves() {
    let limiter = std::sync::Arc::new(RateLimiter::new(10, Duration::from_secs(60)));
    let my_unauthenticated_routes = Router::new().route(
        "/auth/oauth/callback",
        axum::routing::get(|| async { StatusCode::OK }),
    );
    let my_routes = apply_body_limit_layer(apply_observability_layers(apply_rate_limit_layer(
        my_unauthenticated_routes,
        limiter,
    )));
    // Stands in for `build_app_with_rate_limiter(state, web_dir, rate_limiter)`'s return type
    // (`Router`, i.e. `Router<()>`) without needing a live `AppState`/DB.
    let community_app: Router =
        Router::new().route("/up", axum::routing::get(|| async { StatusCode::OK }));
    let app = my_routes.merge(community_app);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/callback")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
