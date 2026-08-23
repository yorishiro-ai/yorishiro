//! Verifies `HostedApp::seed`'s official-templates publisher tenant does not make base's `/setup` wizard read as already set up.

use loco_rs::app::Hooks;
use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_hosted::HostedApp;

/// Sets `YORISHIRO_MAX_TENANTS` for the duration of the future, restoring whatever was there before.
async fn with_max_tenants<T>(value: &str, fut: impl std::future::Future<Output = T>) -> T {
    let previous = std::env::var("YORISHIRO_MAX_TENANTS").ok();
    // SAFETY: serialized by every test in this binary being #[serial] on the default key.
    unsafe {
        std::env::set_var("YORISHIRO_MAX_TENANTS", value);
    }
    let result = fut.await;
    unsafe {
        match &previous {
            Some(v) => std::env::set_var("YORISHIRO_MAX_TENANTS", v),
            None => std::env::remove_var("YORISHIRO_MAX_TENANTS"),
        }
    }
    result
}

#[tokio::test]
#[serial]
async fn setup_still_works_after_hooks_seed_has_run() {
    with_max_tenants("1", async move {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            HostedApp::seed(&ctx, std::path::Path::new("does-not-need-to-exist"))
                .await
                .expect("Hooks::seed");

            let status = request.get("/setup/status").await;
            assert_eq!(status.status_code(), 200);
            let body: serde_json::Value = status.json();
            assert_eq!(
                body["setup_required"], true,
                "the official tenant Hooks::seed creates must not count against the wizard: {body}"
            );

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

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}
