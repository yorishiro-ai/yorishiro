use super::boot_request;
use axum::http::StatusCode;
use yorishiro::App;
#[tokio::test]
async fn signup_without_invite_creates_its_own_tenant() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        request
            .post("/auth/signup")
            .json(&serde_json::json!({"email":"weak@example.com", "password":"short"}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
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
        assert!(signup_body.get("role").is_none(), "signup no longer reports a role label");
        assert_eq!(signup_body["workspaces"].as_array().unwrap().len(), 0);

        let login_response = request
            .post("/auth/root/login")
            .json(&serde_json::json!({
                "email": "no-invite@example.com",
                "password": "correct-horse-battery-staple",
            }))
            .await;
        assert_eq!(login_response.status_code(), StatusCode::OK);
        let login = login_response.json::<serde_json::Value>();
        assert_eq!(login["tenant_id"], signup_body["tenant_id"]);
        assert!(login.get("workspace_id").is_none());
        request.post("/auth/login")
            .json(&serde_json::json!({"email":"no-invite@example.com", "password":"correct-horse-battery-staple"}))
            .await.assert_status(StatusCode::NOT_FOUND);
    })
    .await;
}
/// `YORISHIRO_MAX_TENANTS` bounds invite-less signup the same way it bounds `/setup`: once the cap is reached, further signups are refused rather than growing the tenant count unbounded.
#[tokio::test]
async fn signup_without_invite_respects_the_tenant_cap() {
    let _cap = crate::requests::ConfigOverride::max_tenants(1);
    boot_request::<App, _, _>(|request, _ctx| async move {
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
}

#[tokio::test]
async fn signup_cannot_raise_the_community_tenant_cap_with_configuration() {
    if !crate::require_postgres_backend() {
        return;
    }
    let _licence = crate::requests::ConfigOverride::unlicensed();
    let _cap = crate::requests::ConfigOverride::max_tenants(100);
    boot_request::<App, _, _>(|request, _ctx| async move {
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
}

/// Widens the connection pool past `config/test.yaml`'s default, which would starve the second connection before it ever reaches the advisory lock.
async fn with_db_max_connections<T>(value: u64, fut: impl Future<Output = T>) -> T {
    let _config =
        crate::requests::ConfigOverride::set(&[("database.max_connections", value.into())]);
    fut.await
}
/// `db::lock_for_update` is what serializes `create_tenant`'s cap check; this fails without it.
/// This test is PostgreSQL-only: `lock_for_update` is a no-op on SQLite,
/// so the serialisation guarantee does not hold.
#[tokio::test]
async fn create_tenant_serializes_on_its_advisory_lock() {
    if !crate::require_postgres_backend() {
        return;
    }
    with_db_max_connections(4, async move {
        boot_request::<App, _, _>(|_request, ctx| async move {
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
#[tokio::test]
async fn login_issues_tenant_key_after_credentials_are_verified() {
    use sea_orm::TransactionTrait;
    boot_request::<App, _, _>(|request, ctx| async move {
        let args = super::fixtures::TenantArgs::default();
        let email = args.owner_email.clone();
        let password = args.owner_password.clone();
        let tenant = super::fixtures::TestTenant::create(&request, &ctx, args).await;
        let transaction = ctx.db.begin().await.unwrap();
        let other = yorishiro::models::tenancy::create_workspace(
            &transaction,
            tenant.tenant_id,
            &format!("login-selection-{}", uuid::Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        let response = request
            .post("/auth/root/login")
            .json(&serde_json::json!({"email":email,"password":"wrong-password"}))
            .await;
        assert_eq!(response.status_code(), 401);
        let denied = response.json::<serde_json::Value>();
        assert!(denied["error"]["workspaces"].is_null());
        assert!(denied["error"]["memberships"].is_null());
        let response = request
            .post("/auth/root/login")
            .json(&serde_json::json!({"email":email,"password":password}))
            .await;
        assert_eq!(response.status_code(), 200, "{}", response.text());
        let body = response.json::<serde_json::Value>();
        assert_eq!(body["tenant_id"], tenant.tenant_id.to_string());
        assert!(body.get("workspace_id").is_none());
        assert_ne!(other.id, tenant.workspace_id);
    })
    .await;
}

#[tokio::test]
async fn login_without_membership_is_rejected() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let user = yorishiro::models::user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("no-membership"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        let response = request
            .post("/auth/root/login")
            .json(&serde_json::json!({"email":user.email,"password":"hunter2-hunter2"}))
            .await;
        assert_eq!(response.status_code(), 403);
    })
    .await;
}
