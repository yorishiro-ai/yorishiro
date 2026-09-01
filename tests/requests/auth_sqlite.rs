/// SQLite counterparts for the core auth request tests.
///
/// These exercise the same signup/login flow as `auth.rs` but boot against a
/// SQLite backend, confirming the `AuthContext`/`Authorized<R>` SQLite branches
/// work end to end.
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::tenancy::{self, MembershipRole};

/// Signup via invite then login, then verify replay protection.
///
/// On SQLite this uses `request_with_create_sqlite` to boot against a temp
/// file database with `test_sqlite.yaml` config.
#[tokio::test]
#[serial]
#[ignore = "SQLite request tests: run with --include-ignored"]
async fn signup_then_login_round_trip_sqlite() {
    let db_path = format!("/tmp/yorishiro_auth_test_{}.sqlite3", uuid::Uuid::new_v4());
    super::request_with_create_sqlite::<App, _, _>(db_path.clone(), |request, ctx| async move {
        let tenant = yorishiro::models::_entities::identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("request-test-tenant".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");

        let workspace = yorishiro::models::_entities::identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("request-test-ws".into()),
            status: sea_orm::ActiveValue::Set(
                yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE.to_string(),
            ),
            ..Default::default()
        };
        sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");

        let (_invite, token) = tenancy::create_invite(
            &ctx.db,
            tenant.id,
            "round-trip@example.com",
            MembershipRole::Owner,
            chrono::Duration::hours(1),
        )
        .await
        .expect("create invite");

        let signup_response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "invite_token": token,
                "password": "correct-horse-battery-staple",
                "display_name": "Round Trip",
            }))
            .await;
        assert_eq!(signup_response.status_code(), 201);
        let signup_body: serde_json::Value = signup_response.json();
        assert_eq!(signup_body["email"], "round-trip@example.com");
        assert_eq!(signup_body["role"], "owner");

        let login_response = request
            .post("/auth/login")
            .json(&serde_json::json!({
                "email": "round-trip@example.com",
                "password": "correct-horse-battery-staple",
            }))
            .await;
        assert_eq!(login_response.status_code(), 200);
        let login_body: serde_json::Value = login_response.json();
        assert!(login_body["api_key"].as_str().unwrap().starts_with("ysr_"));
        assert_eq!(login_body["scope"], "migration");

        // The invite is single-use: redeeming the same token again must fail.
        let replay_response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "invite_token": token,
                "password": "irrelevant",
                "display_name": null,
            }))
            .await;
        assert_eq!(replay_response.status_code(), 422);

        // A wrong password on an otherwise-valid account must not leak whether
        // the account exists differently than a truly unknown email would.
        let bad_password_response = request
            .post("/auth/login")
            .json(&serde_json::json!({
                "email": "round-trip@example.com",
                "password": "not-the-password",
            }))
            .await;
        assert_eq!(bad_password_response.status_code(), 401);

        super::close_app_pools_sqlite(&ctx, &db_path).await;
    })
    .await;
}

/// Omitting `invite_token` creates a fresh tenant and joins the caller as `Owner`.
/// No workspace is created, so this account cannot log in until an operator runs
/// `create_workspace` then `create_api_key`.
#[tokio::test]
#[serial]
#[ignore = "SQLite request tests: run with --include-ignored"]
async fn signup_without_invite_creates_its_own_tenant_sqlite() {
    let db_path = format!("/tmp/yorishiro_auth_test_{}.sqlite3", uuid::Uuid::new_v4());
    super::request_with_create_sqlite::<App, _, _>(db_path.clone(), |request, ctx| async move {
        let signup_response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "email": "no-invite@example.com",
                "password": "correct-horse-battery-staple",
                "display_name": "No Invite",
            }))
            .await;
        assert_eq!(
            signup_response.status_code(),
            201,
            "response: {:?}",
            signup_response.text()
        );
        let signup_body: serde_json::Value = signup_response.json();
        assert_eq!(signup_body["email"], "no-invite@example.com");
        assert_eq!(signup_body["role"], "owner");
        assert_eq!(signup_body["workspaces"].as_array().unwrap().len(), 0);

        let login_response = request
            .post("/auth/login")
            .json(&serde_json::json!({
                "email": "no-invite@example.com",
                "password": "correct-horse-battery-staple",
            }))
            .await;
        assert_eq!(
            login_response.status_code(),
            403,
            "response: {:?}",
            login_response.text()
        );

        super::close_app_pools_sqlite(&ctx, &db_path).await;
    })
    .await;
}
