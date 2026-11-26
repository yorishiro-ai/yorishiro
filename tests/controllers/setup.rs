use crate::requests::boot_request_with;
use axum::http::StatusCode;
use sea_orm::{EntityTrait, PaginatorTrait};
use yorishiro::App;

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_setup_creates_only_one_account() {
    let overrides = [crate::requests::max_tenants(1)];
    boot_request_with::<App, _, _>(&overrides, |request, ctx| async move {
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
}

#[tokio::test]
async fn failed_setup_rolls_back_before_a_valid_retry() {
    let overrides = [crate::requests::max_tenants(1)];
    boot_request_with::<App, _, _>(&overrides, |request, ctx| async move {
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
}

/// The source default enables first-run setup with a single-tenant cap, even when the environment does not define an override.
#[tokio::test]
async fn setup_uses_the_single_tenant_source_default() {
    let overrides = [
        crate::requests::max_tenants(0),
        crate::requests::unlicensed(),
    ];
    boot_request_with::<App, _, _>(&overrides, |request, _ctx| async move {
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
        response.assert_status(StatusCode::CREATED);
    })
    .await;
}

/// `POST /setup` creates the first tenant and owner without a workspace and returns a working access key.
/// A second call must be refused with 409 rather than creating a second tenant, and `GET /setup/status` must report `setup_required: false` once the first call has landed.
#[tokio::test]
async fn setup_bootstraps_once_and_refuses_a_second_call() {
    let overrides = [crate::requests::max_tenants(1)];
    boot_request_with::<App, _, _>(&overrides, |request, ctx| async move {
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
        response.assert_status(StatusCode::CREATED);
        let body: serde_json::Value = response.json();
        assert_eq!(body["email"], "owner@example.com");
        {
            use yorishiro::models::iam_tenant_roots;
            let tenant_id: uuid::Uuid = body["tenant_id"].as_str().unwrap().parse().unwrap();
            let root = iam_tenant_roots::Entity::find_by_id(tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("setup creates a tenant root");
            assert_eq!(root.user_id.to_string(), body["user_id"].as_str().unwrap());
        }
        let auth = format!("Bearer {}", body["access_key"].as_str().unwrap());

        assert!(body.get("workspace_id").is_none());
        let workspaces = request
            .get("/api/workspaces")
            .add_header("Authorization", &auth)
            .await;
        workspaces.assert_status_ok();
        assert_eq!(
            workspaces.json::<serde_json::Value>(),
            serde_json::json!([])
        );
        let created = request
            .post("/api/workspaces")
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"name": "first-workspace"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let workspace = created.json::<serde_json::Value>();
        let workspace_id = workspace["id"].as_str().unwrap();

        // Tenant-scoped keys select an explicitly created workspace through the header.
        let caller_identity = request
            .get("/api/caller-identity")
            .add_header("Authorization", &auth)
            .add_header("x-workspace-id", workspace_id)
            .await;
        assert_eq!(caller_identity.status_code(), StatusCode::OK);
        let identity: serde_json::Value = caller_identity.json();
        assert_eq!(identity["workspace_id"], workspace_id);
        assert_eq!(identity["identity_kind"], "user");
        let audit_log = request
            .get("/api/audit-log")
            .add_header("Authorization", &auth)
            .add_header("x-workspace-id", workspace_id)
            .await;
        audit_log.assert_status_ok();

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
}
