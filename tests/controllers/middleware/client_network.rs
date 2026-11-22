use axum::http::StatusCode;
use serde_json::json;
use yorishiro::App;

use crate::requests::boot_request_with;

#[tokio::test]
async fn configured_network_blocks_all_public_transports_from_the_same_peer() {
    let hosts = ("settings.http_allowed_networks", json!(["192.0.2.0/24"]));
    boot_request_with::<App, _, _>(&[hosts], |request, _| async move {
        for path in ["/docs/", "/docs/openapi.json", "/api/schemas", "/mcp"] {
            request
                .get(path)
                .add_header("X-Forwarded-For", "192.0.2.1")
                .await
                .assert_status(StatusCode::FORBIDDEN);
        }
        request.get("/_ping").await.assert_status_ok();
    })
    .await;
}

#[tokio::test]
async fn allowed_peer_reaches_docs_but_still_needs_an_api_key() {
    let hosts = (
        "settings.http_allowed_networks",
        json!(["127.0.0.0/8", "::1/128"]),
    );
    boot_request_with::<App, _, _>(&[hosts], |request, _| async move {
        request.get("/docs/openapi.json").await.assert_status_ok();
        request
            .get("/api/schemas")
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}
