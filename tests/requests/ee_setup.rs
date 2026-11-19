//! Verifies `App::seed`'s official-templates publisher tenant does not make base's `/setup` wizard read as already set up.

use super::boot_request;
use axum::http::StatusCode;
use loco_rs::app::Hooks;
use yorishiro::App;

#[tokio::test]
async fn setup_still_works_after_hooks_seed_has_run() {
    let _cap = crate::requests::ConfigOverride::max_tenants(1);
    boot_request::<App, _, _>(|request, ctx| async move {
        App::seed(&ctx, std::path::Path::new("does-not-need-to-exist"))
            .await
            .expect("Hooks::seed");

        let status = request.get("/setup/status").await;
        assert_eq!(status.status_code(), StatusCode::OK);
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
    })
    .await;
}

#[tokio::test]
async fn unlicensed_ee_cannot_raise_the_tenant_cap() {
    if !crate::require_postgres_backend() {
        return;
    }
    let _licence = crate::requests::ConfigOverride::unlicensed();
    let _cap = crate::requests::ConfigOverride::max_tenants(100);
    boot_request::<App, _, _>(|request, _ctx| async move {
            let first = request
                .post("/auth/signup")
                .json(&serde_json::json!({"email":"first-ee@example.com","password":"correct-horse-battery-staple"}))
                .await;
            assert_eq!(first.status_code(), StatusCode::CREATED);
            let second = request
                .post("/auth/signup")
                .json(&serde_json::json!({"email":"second-ee@example.com","password":"correct-horse-battery-staple"}))
                .await;
            assert_eq!(second.status_code(), StatusCode::CONFLICT);
        })
        .await;
}
