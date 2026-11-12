use axum::http::StatusCode;
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;
use yorishiro::App;

use super::boot_request_isolated;
use super::with_max_tenants;

#[tokio::test(flavor = "multi_thread")]
#[serial(process_environment)]
async fn concurrent_setup_creates_only_one_account() {
    with_max_tenants("1", async {
        boot_request_isolated::<App, _, _>(|request, ctx| async move {
            let first = request.post("/setup").json(&serde_json::json!({
                "email": "first@example.com", "password": "hunter2-hunter2",
            }));
            let second = request.post("/setup").json(&serde_json::json!({
                "email": "second@example.com", "password": "hunter2-hunter2",
            }));
            let (first, second) = tokio::join!(first, second);
            let mut statuses = [first.status_code(), second.status_code()];
            statuses.sort();
            assert_eq!(
                statuses,
                [StatusCode::CREATED, StatusCode::CONFLICT],
                "first: {}; second: {}",
                first.text(),
                second.text()
            );
            assert_eq!(
                yorishiro::models::tenant_tenants::Entity::find()
                    .count(&ctx.db)
                    .await
                    .unwrap(),
                1
            );
        })
        .await;
    })
    .await;
}

#[tokio::test]
#[serial(process_environment)]
async fn failed_setup_rolls_back_before_a_valid_retry() {
    with_max_tenants("1", async {
        boot_request_isolated::<App, _, _>(|request, ctx| async move {
            yorishiro::models::user_users::create_user(
                &ctx.db,
                "taken@example.com",
                "hunter2-hunter2",
                None,
            )
            .await
            .unwrap();
            let invalid = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "taken@example.com", "password": "hunter2-hunter2",
                }))
                .await;
            assert_eq!(invalid.status_code(), StatusCode::CONFLICT);
            assert_eq!(
                yorishiro::models::tenant_tenants::Entity::find()
                    .count(&ctx.db)
                    .await
                    .unwrap(),
                0
            );
            let valid = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "valid@example.com", "password": "hunter2-hunter2",
                }))
                .await;
            assert_eq!(valid.status_code(), StatusCode::CREATED, "{}", valid.text());
        })
        .await;
    })
    .await;
}

/// The source default enables first-run setup with a single-tenant cap, even when the environment does not define an override.
#[tokio::test]
#[serial(process_environment)]
async fn setup_uses_the_single_tenant_source_default() {
    let ambient = crate::EnvGuard::capture(&[
        crate::env::TestEnv::MaxTenants.name(),
        crate::env::TestEnv::LicenseKey.name(),
    ]);
    ambient.remove(crate::env::TestEnv::MaxTenants.name());
    ambient.remove(crate::env::TestEnv::LicenseKey.name());
    boot_request_isolated::<App, _, _>(|request, _ctx| async move {
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
        boot_request_isolated::<App, _, _>(|request, _ctx| async move {
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

            // The tenant-scoped key requires the returned workspace ID in the header.
            let whoami = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .add_header("x-workspace-id", body["workspace_id"].as_str().unwrap())
                .await;
            assert_eq!(whoami.status_code(), StatusCode::OK);
            let identity: serde_json::Value = whoami.json();
            assert_eq!(identity["workspace_id"], body["workspace_id"]);
            assert_eq!(identity["stored_scope"], "migration");
            assert_eq!(identity["scope"], "migration");
            assert_eq!(identity["audit"], true);
            assert_eq!(identity["identity_kind"], "user");
            assert!(identity["bound_workspace_id"].is_null());
            let audit_log = request
                .get("/api/audit-log")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .add_header("x-workspace-id", body["workspace_id"].as_str().unwrap())
                .await;
            assert_eq!(
                audit_log.status_code(),
                StatusCode::OK,
                "{}",
                audit_log.text()
            );

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

/// The setup key does not select a workspace implicitly; workspace-scoped keys retain their binding.
#[tokio::test]
#[serial(process_environment)]
async fn setup_key_resolves_the_workspace_per_request() {
    with_max_tenants("1", async move {
        boot_request_isolated::<App, _, _>(|request, ctx| async move {
            let response = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "owner@example.com",
                    "password": "hunter2-hunter2",
                }))
                .await;
            assert_eq!(response.status_code(), 201, "{}", response.text());
            let body: serde_json::Value = response.json();
            let api_key = body["api_key"].as_str().unwrap().to_string();
            let workspace_id: uuid::Uuid = body["workspace_id"].as_str().unwrap().parse().unwrap();

            // Even with one workspace, tenant authentication does not select it.
            let no_workspace = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await;
            no_workspace.assert_status(StatusCode::OK);
            assert!(no_workspace.json::<serde_json::Value>()["workspace_id"].is_null());
            request
                .get("/api/tenants/current")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await
                .assert_status(StatusCode::OK);
            request
                .get("/api/schemas")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await
                .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
            let whoami = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .add_header("x-workspace-id", workspace_id.to_string())
                .await;
            assert_eq!(whoami.status_code(), StatusCode::OK, "{}", whoami.text());
            let identity: serde_json::Value = whoami.json();
            assert_eq!(identity["workspace_id"], workspace_id.to_string());

            // A second workspace can also be selected explicitly.
            let created = request
                .post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .add_header("x-workspace-id", workspace_id.to_string())
                .json(&serde_json::json!({ "name": "Second" }))
                .await;
            assert_eq!(created.status_code(), 201, "{}", created.text());
            let second_id: uuid::Uuid = created.json::<serde_json::Value>()["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();

            let ambiguous = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await;
            assert_eq!(ambiguous.status_code(), 200, "{}", ambiguous.text());
            assert!(ambiguous.json::<serde_json::Value>()["workspace_id"].is_null());

            for workspace in [workspace_id, second_id] {
                let whoami = request
                    .get("/api/whoami")
                    .add_header("Authorization", format!("Bearer {api_key}"))
                    .add_header("x-workspace-id", workspace.to_string())
                    .await;
                assert_eq!(whoami.status_code(), 200, "{}", whoami.text());
                let chosen: serde_json::Value = whoami.json();
                assert_eq!(chosen["workspace_id"], workspace.to_string());
            }

            // A workspace-scoped key acts on its own workspace and refuses a foreign header.
            let user_id: uuid::Uuid = identity["user_id"].as_str().unwrap().parse().unwrap();
            let scoped_key = yorishiro::models::api_keys::Entity::create_api_key(
                &ctx.db,
                workspace_id,
                yorishiro::models::api_keys::ApiKeyScope::Read,
                Some(user_id),
                false,
            )
            .await
            .unwrap();
            request
                .get("/api/tenants/current")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await
                .assert_status(StatusCode::OK);
            // A foreign header is a scope denial (403), not a validation failure (422).
            let scoped = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .add_header("x-workspace-id", second_id.to_string())
                .await;
            assert_eq!(scoped.status_code(), 403, "{}", scoped.text());
            let scoped = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await;
            assert_eq!(scoped.status_code(), 200, "{}", scoped.text());
            let identity: serde_json::Value = scoped.json();
            assert_eq!(identity["workspace_id"], workspace_id.to_string());
        })
        .await;
    })
    .await;
}
