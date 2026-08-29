use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro::app::App;

use super::with_max_tenants;

/// With no cap set, both endpoints must answer as if setup does not exist, rather than leaking whether a tenant exists on a deployment that never opted into the wizard at all.
///
/// This covers `App` rather than the shipped base binary, and the distinction matters since `src/bin/main.rs` gained its single-tenant default.
/// `config/test.yaml` sets no `YORISHIRO_MAX_TENANTS`, and this harness boots `App` directly through `request_with_create_db` without going through any binary's `main`, so the prologue that would set the variable never runs here.
/// A base deployment started from the binary therefore has the wizard *enabled*, which is the opposite of what this test asserts and is not a contradiction: the two exercise different entry points.
/// Nothing in this suite invokes a binary, so the prologue itself is deliberately untested rather than overlooked; testing it would mean building a harness around `main` for four lines.
#[tokio::test]
#[serial]
async fn setup_is_unreachable_when_no_tenant_cap_is_set() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let status = request.get("/setup/status").await;
        assert_eq!(status.status_code(), 200);
        let body: serde_json::Value = status.json();
        assert_eq!(body["setup_required"], false);

        let response = request
            .post("/setup")
            .json(&serde_json::json!({
                "email": "owner@example.com",
                "password": "hunter2-hunter2",
            }))
            .await;
        assert_eq!(
            response.status_code(),
            404,
            "response: {:?}",
            response.text()
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// With the wizard enabled and no tenant yet, `POST /setup` creates the deployment's first tenant/workspace/owner and returns a working API key.
/// A second call must be refused with 409 rather than creating a second tenant, and `GET /setup/status` must report `setup_required: false` once the first call has landed.
#[tokio::test]
#[serial]
async fn setup_bootstraps_once_and_refuses_a_second_call() {
    with_max_tenants("1", async move {
        request_with_create_db::<App, _, _>(|request, ctx| async move {
            let status = request.get("/setup/status").await;
            assert_eq!(status.status_code(), 200);
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
            assert_eq!(whoami.status_code(), 200);

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
            assert_eq!(second.status_code(), 409, "response: {:?}", second.text());

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}
