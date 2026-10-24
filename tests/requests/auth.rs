use super::fixtures::{TenantArgs, TestTenant};
use super::{boot_request, boot_request_isolated};
use axum::http::StatusCode;
use serial_test::serial;
use yorishiro::App;
use yorishiro::models::tenant_memberships::MembershipRole;

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_invited_signups_consume_the_token_once_without_internal_errors() {
    let race_invite_email = super::fixtures::unique_email("race-invite");
    use yorishiro::models::workspace_invites;

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let (_, token) = workspace_invites::create_invite(
            &ctx.db,
            tenant.tenant_id,
            race_invite_email.as_str(),
            MembershipRole::Member,
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();
        let body = serde_json::json!({"invite_token": token, "password": "test-password"});
        let (first, second) = tokio::join!(
            request.post("/auth/signup").json(&body),
            request.post("/auth/signup").json(&body),
        );
        let statuses = [first.status_code(), second.status_code()];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CREATED)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::UNPROCESSABLE_ENTITY)
                .count(),
            1,
            "responses: {} / {}",
            first.text(),
            second.text()
        );
    })
    .await;
}

#[tokio::test]
async fn failed_invited_registration_does_not_consume_the_invite() {
    let exists_email = super::fixtures::unique_email("exists");
    use sea_orm::TransactionTrait;
    use yorishiro::models::{user_users, workspace_invites};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        user_users::create_user(&ctx.db, exists_email.as_str(), "password", None)
            .await
            .expect("existing user");
        let (_, token) = workspace_invites::create_invite(
            &ctx.db,
            tenant.tenant_id,
            exists_email.as_str(),
            MembershipRole::Member,
            chrono::Duration::hours(1),
        )
        .await
        .expect("invite");

        let response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "invite_token": token, "password": "password"
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::CONFLICT);
        let txn = ctx.db.begin().await.expect("begin invite check");
        let redeemed = workspace_invites::redeem_invite_at(&txn, &token, chrono::Utc::now())
            .await
            .expect("redeem after failed signup")
            .expect("failed signup must leave the invite usable");
        assert_eq!(redeemed.tenant_id, tenant.tenant_id);
        assert_eq!(redeemed.email, exists_email);
        txn.rollback().await.expect("rollback invite check");
    })
    .await;
}

#[tokio::test]
async fn signup_then_login_round_trip() {
    let round_trip_email = super::fixtures::unique_email("round-trip");
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let (_invite, token) = yorishiro::models::workspace_invites::create_invite(
            &ctx.db,
            tenant.tenant_id,
            round_trip_email.as_str(),
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
        assert_eq!(signup_response.status_code(), StatusCode::CREATED);
        let signup_body: serde_json::Value = signup_response.json();
        assert_eq!(signup_body["email"], round_trip_email.as_str());
        assert_eq!(signup_body["role"], "owner");

        let login_response = request
            .post("/auth/login")
            .json(&serde_json::json!({
                "email": round_trip_email.as_str(),
                "password": "correct-horse-battery-staple",
            }))
            .await;
        assert_eq!(login_response.status_code(), StatusCode::OK);
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
        assert_eq!(
            replay_response.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        // A wrong password on an otherwise-valid account must not leak whether
        // the account exists differently than a truly unknown email would.
        let bad_password_response = request
            .post("/auth/login")
            .json(&serde_json::json!({
                "email": round_trip_email.as_str(),
                "password": "not-the-password",
            }))
            .await;
        assert_eq!(
            bad_password_response.status_code(),
            StatusCode::UNAUTHORIZED
        );
    })
    .await;
}

#[tokio::test]
async fn signup_without_invite_creates_its_own_tenant() {
    boot_request_isolated::<App, _, _>(|request, _ctx| async move {
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
async fn signup_rejects_email_alongside_invite_token() {
    let should_not_be_here_email = super::fixtures::unique_email("should-not-be-here");
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "invite_token": "irrelevant-since-this-should-422-first",
                "email": should_not_be_here_email.as_str(),
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
async fn signup_rejects_neither_invite_token_nor_email() {
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
#[serial(process_environment)]
async fn signup_without_invite_respects_the_tenant_cap() {
    super::with_max_tenants("1", async move {
        boot_request_isolated::<App, _, _>(|request, _ctx| async move {
            let first = request
                .post("/auth/signup")
                .json(&serde_json::json!({
                    "email": "first@example.com",
                    "password": "correct-horse-battery-staple",
                }))
                .await;
            assert_eq!(
                first.status_code(),
                StatusCode::CREATED,
                "response: {:?}",
                first.text()
            );

            let second = request
                .post("/auth/signup")
                .json(&serde_json::json!({
                    "email": "second@example.com",
                    "password": "correct-horse-battery-staple",
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

#[tokio::test]
#[serial(process_environment)]
async fn signup_cannot_raise_the_community_tenant_cap_with_configuration() {
    if !crate::require_postgres_backend() {
        return;
    }
    let guard = crate::EnvGuard::capture(&["YORISHIRO_LICENSE_KEY"]);
    guard.remove("YORISHIRO_LICENSE_KEY");
    super::with_max_tenants("100", async move {
        boot_request_isolated::<App, _, _>(|request, _ctx| async move {
            for email in ["first@example.com", "second@example.com"] {
                let response = request
                    .post("/auth/signup")
                    .json(&serde_json::json!({
                        "email": email,
                        "password": "correct-horse-battery-staple",
                    }))
                    .await;
                if email.starts_with("first") {
                    assert_eq!(response.status_code(), StatusCode::CREATED);
                } else {
                    assert_eq!(response.status_code(), StatusCode::CONFLICT);
                }
            }
        })
        .await;
    })
    .await;
}

/// Widens `DB_MAX_CONNECTIONS` past `config/test_postgres.yaml`'s default of 1, which would starve the second connection before it ever reaches the advisory lock.
async fn with_db_max_connections<T>(value: &str, fut: impl std::future::Future<Output = T>) -> T {
    let guard = crate::EnvGuard::capture(&["DB_MAX_CONNECTIONS"]);
    guard.set("DB_MAX_CONNECTIONS", value);
    fut.await
}
/// `db::lock_for_update` is what serializes `create_tenant`'s cap check; this fails without it.
/// This test is PostgreSQL-only: `lock_for_update` is a no-op on SQLite,
/// so the serialisation guarantee does not hold.
#[tokio::test]
#[serial(process_environment)]
async fn create_tenant_serializes_on_its_advisory_lock() {
    if !super::super::require_postgres_backend() {
        return;
    }
    with_db_max_connections("4", async move {
        boot_request_isolated::<App, _, _>(|_request, ctx| async move {
            let holder = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
            yorishiro::db::lock_for_update(&holder, "create_tenant")
                .await
                .unwrap();

            // A second caller must block behind the held lock, not proceed concurrently.
            let db = ctx.db.clone();
            let blocked = tokio::time::timeout(std::time::Duration::from_millis(500), async move {
                let txn = sea_orm::TransactionTrait::begin(&db).await.unwrap();
                let result = yorishiro::models::tenant_tenants::create_tenant(
                    &txn,
                    "blocked-racer",
                    Some(100),
                )
                .await;
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
            let result = yorishiro::models::tenant_tenants::create_tenant(
                &txn,
                "unblocked-racer",
                Some(100),
            )
            .await;
            assert!(result.is_ok(), "result: {result:?}");
            txn.commit().await.unwrap();
        })
        .await;
    })
    .await;
}
