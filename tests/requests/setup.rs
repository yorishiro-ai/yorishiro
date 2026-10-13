use axum::http::StatusCode;
use serial_test::serial;
use yorishiro::App;

use super::boot_request;
use super::with_max_tenants;

/// The source default enables first-run setup with a single-tenant cap, even when the environment does not define an override.
#[tokio::test]
#[serial(process_environment)]
async fn setup_uses_the_single_tenant_source_default() {
    let ambient = crate::EnvGuard::capture(&["YORISHIRO_MAX_TENANTS"]);
    ambient.remove("YORISHIRO_MAX_TENANTS");
    boot_request::<App, _, _>(|request, _ctx| async move {
        let status = request.get("/setup/status").await;
        assert_eq!(status.status_code(), StatusCode::OK);
        let body: serde_json::Value = status.json();
        assert_eq!(body["setup_required"], true);

        let response = request
            .post("/setup")
            .json(&serde_json::json!({
                "email": "owner@example.com",
                "password": "hunter2-hunter2",
            }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// With the wizard enabled and no tenant yet, `POST /setup` creates the deployment's first tenant/workspace/owner and returns a working API key.
/// A second call must be refused with 409 rather than creating a second tenant, and `GET /setup/status` must report `setup_required: false` once the first call has landed.
#[tokio::test]
#[serial(process_environment)]
async fn setup_bootstraps_once_and_refuses_a_second_call() {
    with_max_tenants("1", async move {
        boot_request::<App, _, _>(|request, _ctx| async move {
            let status = request.get("/setup/status").await;
            assert_eq!(status.status_code(), StatusCode::OK);
            let body: serde_json::Value = status.json();
            assert_eq!(body["setup_required"], true, "body: {body}");

            let response = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "owner@example.com",
                    "password": "hunter2-hunter2",
                    "display_name": "Owner",
                }))
                .await;
            assert_eq!(
                response.status_code(),
                201,
                "response: {:?}",
                response.text()
            );
            let body: serde_json::Value = response.json();
            assert_eq!(body["email"], "owner@example.com");
            let api_key = body["api_key"].as_str().unwrap().to_string();

            // The issued key actually works.
            let whoami = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await;
            assert_eq!(whoami.status_code(), StatusCode::OK);

            let status = request.get("/setup/status").await;
            let body: serde_json::Value = status.json();
            assert_eq!(
                body["setup_required"], false,
                "setup_required must flip once a tenant exists: {body}"
            );

            // A second call, anonymous, must not be able to create a second tenant.
            let second = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "attacker@example.com",
                    "password": "hunter2-hunter2",
                }))
                .await;
            assert_eq!(
                second.status_code(),
                StatusCode::CONFLICT,
                "response: {:?}",
                second.text()
            );
        })
        .await;
    })
    .await;
}
