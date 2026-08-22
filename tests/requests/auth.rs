use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_core::app::App;
use yorishiro_core::models::tenancy::{self, MembershipRole};

#[tokio::test]
#[serial]
async fn signup_then_login_round_trip() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let tenant = yorishiro_core::models::_entities::identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("request-test-tenant".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");

        let workspace = yorishiro_core::models::_entities::identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("request-test-ws".into()),
            status: sea_orm::ActiveValue::Set(
                yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE.to_string(),
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

        // The invite is single-use: redeeming the same token again must fail rather than
        // silently creating a second account. This is a sequential replay, not a concurrent
        // one: it does not exercise redeem_invite's race-safety claim (two redemptions racing
        // on the same UPDATE ... WHERE used_at IS NULL), only that a used invite is rejected on
        // a later, separate request.
        let replay_response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "invite_token": token,
                "password": "irrelevant",
                "display_name": null,
            }))
            .await;
        assert_eq!(replay_response.status_code(), 422);

        // A wrong password on an otherwise-valid account must not leak whether the account
        // exists differently than a truly unknown email would.
        let bad_password_response = request
            .post("/auth/login")
            .json(&serde_json::json!({
                "email": "round-trip@example.com",
                "password": "not-the-password",
            }))
            .await;
        assert_eq!(bad_password_response.status_code(), 401);

        close_app_pools(&ctx).await;
    })
    .await;
}

/// `after_context` opens two pools Loco's own request-test harness knows nothing about: the
/// identity pool (eager, holds a session immediately) and the tenant pool (lazy, only opened on
/// first use). Neither closing means a session survives on the throwaway test database, and
/// `request_with_create_db`'s teardown does `DROP DATABASE`, which fails on any surviving
/// session. `ctx.db` itself also needs closing: `config/test.yaml`'s `min_connections: 1` keeps
/// one connection open from boot. `queue_provider` is not closed here: `config/test.yaml` has no
/// `queue:` block (see its comment), so it is always `None` in this environment; the Postgres
/// queue provider's `shutdown()` only cancels its polling loop, not its own `PgPool`, and has no
/// method that does, which is exactly why the queue stays off for tests rather than being closed
/// here.
///
/// Every request test that runs through `request_with_create_db` must call this before its
/// closure returns.
async fn close_app_pools(ctx: &loco_rs::app::AppContext) {
    if let Some(db) = ctx.shared_store.get::<yorishiro_core::db::DbHandle>() {
        db.identity.close().await;
        db.tenant.pool().close().await;
    }
    ctx.db.get_postgres_connection_pool().close().await;
}
