use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_hosted::HostedApp;

/// `YORISHIRO_MAX_TENANTS` unset means the setup wizard is disabled by default (same guard `tests/requests/setup.rs` in the base crate exercises): this test sets it for its own duration, matching that file's `with_max_tenants` convention, kept local since this crate has no shared helper for it yet and one test doesn't earn one.
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

/// The freshly bootstrapped owner can read their own tenant's overview: zero usage, no plan
/// (never subscribed), and themselves as the sole member.
#[tokio::test]
#[serial]
async fn tenant_overview_returns_usage_and_members_for_the_owner() {
    with_max_tenants("1", async move {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let setup = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "owner@example.com",
                    "password": "hunter2-hunter2",
                    "display_name": "Owner",
                }))
                .await;
            assert_eq!(setup.status_code(), 201, "response: {:?}", setup.text());
            let setup_body: serde_json::Value = setup.json();
            let api_key = setup_body["api_key"].as_str().unwrap();

            let response = request
                .get("/hosted/tenant/overview")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await;
            assert_eq!(
                response.status_code(),
                200,
                "response: {:?}",
                response.text()
            );
            let body: serde_json::Value = response.json();
            assert_eq!(body["plan"], serde_json::Value::Null);
            assert_eq!(body["usage"]["workspace_count"], 1);
            assert_eq!(body["usage"]["member_count"], 1);
            assert_eq!(body["members"].as_array().unwrap().len(), 1);
            assert_eq!(body["members"][0]["email"], "owner@example.com");
            assert_eq!(body["members"][0]["role"], "owner");

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// No `Authorization` header at all must be rejected, not treated as an anonymous tenant.
#[tokio::test]
#[serial]
async fn tenant_overview_requires_authentication() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let response = request.get("/hosted/tenant/overview").await;
        assert_eq!(
            response.status_code(),
            401,
            "response: {:?}",
            response.text()
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}
