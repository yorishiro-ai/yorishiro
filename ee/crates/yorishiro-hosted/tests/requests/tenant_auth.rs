use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_core::db::DbHandle;
use yorishiro_hosted::HostedApp;
use yorishiro_hosted::services::tenant_auth::{WORKSPACE_HEADER, create_tenant_api_key};

/// `YORISHIRO_MAX_TENANTS` unset means the setup wizard is disabled by default.
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

/// Installing `TenantScopedAuthenticator` (`HostedApp::after_context`) must not break a
/// workspace-scoped key on a route base itself defines.
#[tokio::test]
#[serial]
async fn a_workspace_scoped_key_still_works_on_a_base_route() {
    with_max_tenants("1", async move {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let setup = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "owner@example.com",
                    "password": "hunter2-hunter2",
                }))
                .await;
            assert_eq!(setup.status_code(), 201, "response: {:?}", setup.text());
            let api_key = setup.json::<serde_json::Value>()["api_key"]
                .as_str()
                .unwrap()
                .to_string();

            let response = request
                .get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await;
            assert_eq!(
                response.status_code(),
                200,
                "response: {:?}",
                response.text()
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}

/// A tenant-scoped key (`workspace_id` NULL) names its workspace per request with
/// `X-Workspace-Id`, and is rejected without one.
#[tokio::test]
#[serial]
async fn a_tenant_scoped_key_resolves_the_workspace_named_by_the_header() {
    with_max_tenants("1", async move {
        request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
            let setup = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "owner@example.com",
                    "password": "hunter2-hunter2",
                }))
                .await;
            assert_eq!(setup.status_code(), 201, "response: {:?}", setup.text());
            let setup_body: serde_json::Value = setup.json();
            let tenant_id: uuid::Uuid = setup_body["tenant_id"].as_str().unwrap().parse().unwrap();
            let workspace_id: uuid::Uuid = setup_body["workspace_id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let user_id: uuid::Uuid = setup_body["user_id"].as_str().unwrap().parse().unwrap();

            // create_tenant_api_key reads identity_tenants and identity_tenant_memberships, neither granted to yorishiro_app (see identity_tenants's migration doc comment), so it runs on the identity pool (the migration/admin role), not the RLS-scoped tenant pool.
            let db = ctx.shared_store.get::<DbHandle>().unwrap();
            let tenant_key = create_tenant_api_key(&db.identity, tenant_id, "read", Some(user_id))
                .await
                .expect("issuing a tenant-scoped key");

            // Without the header, `authenticate_api_key(hash, NULL)` finds no workspace to resolve against, so this reaches the caller as an ordinary authentication failure, not a validation error naming the header.
            let no_header = request
                .get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
                .await;
            assert_eq!(
                no_header.status_code(),
                401,
                "response: {:?}",
                no_header.text()
            );

            let with_header = request
                .get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
                .add_header(WORKSPACE_HEADER, workspace_id.to_string())
                .await;
            assert_eq!(
                with_header.status_code(),
                200,
                "response: {:?}",
                with_header.text()
            );

            super::close_app_pools(&ctx).await;
        })
        .await;
    })
    .await;
}
