use super::fixtures::{TenantArgs, TestTenant, issue_api_key};
use super::{boot_request, boot_request_isolated};
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;

#[tokio::test]
async fn maintenance_is_readable_and_settable_over_rest() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let key = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .api_key;

        let response = request
            .get("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert_eq!(body["mode"], "off", "a fresh deployment serves normally");

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "read-only", "reason": "restoring a backup" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert_eq!(body["mode"], "read_only");
        assert_eq!(body["reason"], "restoring a backup");
        assert_eq!(body["retry_after"], 300, "the CLI's default, unchanged");

        let response = request
            .get("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        let body: serde_json::Value = response.json();
        assert_eq!(body["mode"], "read_only", "the write is what the read sees");

        // Restore off so a leaked mutex-held row doesn't affect an unrelated test running against the same throwaway database.
        request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "off" }))
            .await;
    })
    .await;
}

/// Behind the maintenance guard, a full lock entered over REST could only be left over the CLI, which makes the switch a one-way door for anyone without shell access.
/// This test fails if the route ever moves behind the guard.
#[tokio::test]
async fn a_full_lock_entered_over_rest_can_be_left_over_rest() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let key = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .api_key;

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "full-lock" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);

        // Everything else is refused now, which is what full lock means.
        let locked = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(locked.status_code(), StatusCode::SERVICE_UNAVAILABLE);

        // The switch itself still answers, or there would be no way back.
        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "off" }))
            .await;
        assert_eq!(
            response.status_code(),
            200,
            "the maintenance route must answer under full lock"
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["mode"], "off");

        let served = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(
            served.status_code(),
            StatusCode::OK,
            "and the deployment is back"
        );
    })
    .await;
}

/// `migration` is the scope that guards batch migration and the maintenance switch alike.
/// A `write`-scoped key stopping every caller would be an escalation.
#[tokio::test]
async fn a_member_key_cannot_touch_maintenance() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let member = yorishiro::models::user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("member"),
            "hunter2-hunter2",
            None,
        )
        .await
        .expect("create member");
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant.tenant_id,
            member.id,
            MembershipRole::Member,
        )
        .await
        .expect("add member");
        let key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            member.id,
            ApiKeyScope::Write,
            false,
        )
        .await;

        let response = request
            .get("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "full-lock" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
    })
    .await;
}

/// A typo must not read as a mode.
/// Silently ignoring it would leave an operator believing the deployment is locked when it is serving.
#[tokio::test]
async fn an_unknown_mode_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let key = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .api_key;

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "readonly" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: serde_json::Value = response.json();
        assert!(
            body["error"]["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("read-only"),
            "the hint must spell the modes: {body}"
        );
    })
    .await;
}

#[tokio::test]
async fn auth_endpoints_are_rate_limited() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let mut last_status = 200;
        for _ in 0..15 {
            let response = request
                .post("/auth/login")
                .json(&serde_json::json!({ "email": "nobody@example.com", "password": "wrong" }))
                .await;
            last_status = response.status_code().as_u16();
        }
        assert_eq!(
            last_status, 429,
            "YORISHIRO_AUTH_RATE_LIMIT_MAX defaults to 10 per window; 15 attempts must exhaust it"
        );
    })
    .await;
}
