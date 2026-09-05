use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::tenancy::{self, MembershipRole};

use super::boot_request;
use super::with_max_tenants;

#[tokio::test]
#[serial]
async fn signup_then_login_round_trip() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
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

        // A wrong password on an otherwise-valid account must not leak whether the account exists differently than a truly unknown email would.
        let bad_password_response = request
            .post("/auth/login")
            .json(&serde_json::json!({
                "email": "round-trip@example.com",
                "password": "not-the-password",
            }))
            .await;
        assert_eq!(bad_password_response.status_code(), 401);
    })
    .await;
}

/// Omitting `invite_token` creates a fresh tenant and joins the caller as `Owner`.
/// No workspace is created, so this account cannot log in until an operator runs `create_workspace` then `create_api_key`.
#[tokio::test]
#[serial]
async fn signup_without_invite_creates_its_own_tenant() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, _ctx| async move {
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
    })
    .await;
}

/// `email` alongside `invite_token` is rejected rather than silently ignored: the invite already carries the email it was issued to, and a caller-supplied one could disagree with it unnoticed.
#[tokio::test]
#[serial]
async fn signup_rejects_email_alongside_invite_token() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "invite_token": "irrelevant-since-this-should-422-first",
                "email": "should-not-be-here@example.com",
                "password": "correct-horse-battery-staple",
            }))
            .await;
        assert_eq!(
            response.status_code(),
            422,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// No `invite_token` and no `email` has nothing to create a tenant or an account for.
#[tokio::test]
#[serial]
async fn signup_rejects_neither_invite_token_nor_email() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "password": "correct-horse-battery-staple",
            }))
            .await;
        assert_eq!(
            response.status_code(),
            422,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// `YORISHIRO_MAX_TENANTS` bounds invite-less signup the same way it bounds `/setup`: once the cap is reached, further signups are refused rather than growing the tenant count unbounded.
#[tokio::test]
#[serial]
async fn signup_without_invite_respects_the_tenant_cap() {
    with_max_tenants("1", async move {
        if super::super::require_sqlite_backend() {
            return;
        }
        boot_request::<App, _, _>(|request, _ctx| async move {
            let first = request
                .post("/auth/signup")
                .json(&serde_json::json!({
                    "email": "first@example.com",
                    "password": "correct-horse-battery-staple",
                }))
                .await;
            assert_eq!(first.status_code(), 201, "response: {:?}", first.text());

            let second = request
                .post("/auth/signup")
                .json(&serde_json::json!({
                    "email": "second@example.com",
                    "password": "correct-horse-battery-staple",
                }))
                .await;
            assert_eq!(second.status_code(), 409, "response: {:?}", second.text());
        })
        .await;
    })
    .await;
}

/// Widens `DB_MAX_CONNECTIONS` past `config/test.yaml`'s default of 1, which would starve the second connection before it ever reaches the advisory lock.
async fn with_db_max_connections<T>(value: &str, fut: impl std::future::Future<Output = T>) -> T {
    let previous = std::env::var("DB_MAX_CONNECTIONS").ok();
    // SAFETY: serialized by every test in this binary being #[serial] on the default key.
    unsafe {
        std::env::set_var("DB_MAX_CONNECTIONS", value);
    }
    let result = fut.await;
    unsafe {
        match &previous {
            Some(v) => std::env::set_var("DB_MAX_CONNECTIONS", v),
            None => std::env::remove_var("DB_MAX_CONNECTIONS"),
        }
    }
    result
}

/// `db::lock_for_update` is what serializes `create_tenant`'s cap check; this fails without it.
#[tokio::test]
#[serial]
async fn create_tenant_serializes_on_its_advisory_lock() {
    with_max_tenants("100", async move {
        with_db_max_connections("4", async move {
            if super::super::require_sqlite_backend() { return; }
            boot_request::<App, _, _>(|_request, ctx| async move {
                let holder = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
                yorishiro::db::lock_for_update(&holder, "create_tenant")
                    .await
                    .unwrap();

                // A second caller must block behind the held lock, not proceed concurrently.
                let db = ctx.db.clone();
                let blocked =
                    tokio::time::timeout(std::time::Duration::from_millis(500), async move {
                        let txn = sea_orm::TransactionTrait::begin(&db).await.unwrap();
                        let result = tenancy::create_tenant(&txn, "blocked-racer").await;
                        match &result {
                            Ok(_) => txn.commit().await.unwrap(),
                            Err(_) => txn.rollback().await.unwrap(),
                        }
                        result
                    })
                    .await;
                assert!(
                    blocked.is_err(),
                    "a second create_tenant call must block on the held lock, not proceed: {blocked:?}"
                );

                // Releasing the lock lets the next caller through.
                holder.rollback().await.unwrap();
                let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
                let result = tenancy::create_tenant(&txn, "unblocked-racer").await;
                assert!(result.is_ok(), "result: {result:?}");
                txn.commit().await.unwrap();

            })
            .await;
        })
        .await;
    })
    .await;
}
