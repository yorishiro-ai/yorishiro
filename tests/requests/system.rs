use super::boot_request;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};

async fn issue_key_for(
    ctx: &loco_rs::app::AppContext,
    workspace_id: uuid::Uuid,
    user_id: uuid::Uuid,
    role: MembershipRole,
) -> String {
    identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace_id,
        role.max_scope(),
        Some(user_id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext
}

/// An owner's key carries `migration` scope, which is what guards these routes.
async fn owner_key(ctx: &loco_rs::app::AppContext, name: &str) -> String {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(name.to_string()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".to_string()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    issue_key_for(ctx, workspace.id, owner.id, MembershipRole::Owner).await
}

/// A member's key tops out at `write`, which is below `migration`.
async fn member_key(ctx: &loco_rs::app::AppContext, name: &str) -> String {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(name.to_string()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".to_string()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let member = tenancy::create_user(&ctx.db, "member@example.com", "hunter2-hunter2", None)
        .await
        .expect("create member");
    tenancy::add_member(&ctx.db, tenant.id, member.id, MembershipRole::Member)
        .await
        .expect("add member");
    issue_key_for(ctx, workspace.id, member.id, MembershipRole::Member).await
}

#[tokio::test]
#[serial]
async fn maintenance_is_readable_and_settable_over_rest() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let key = owner_key(&ctx, "acme").await;

        let response = request
            .get("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        assert_eq!(body["mode"], "off", "a fresh deployment serves normally");

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "read-only", "reason": "restoring a backup" }))
            .await;
        assert_eq!(response.status_code(), 200);
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
#[serial]
async fn a_full_lock_entered_over_rest_can_be_left_over_rest() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let key = owner_key(&ctx, "acme").await;

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "full-lock" }))
            .await;
        assert_eq!(response.status_code(), 200);

        // Everything else is refused now, which is what full lock means.
        let locked = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(locked.status_code(), 503);

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
        assert_eq!(served.status_code(), 200, "and the deployment is back");
    })
    .await;
}

/// `migration` is the scope that guards batch migration and the maintenance switch alike.
/// A `write`-scoped key stopping every caller would be an escalation.
#[tokio::test]
#[serial]
async fn a_member_key_cannot_touch_maintenance() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let key = member_key(&ctx, "beta").await;

        let response = request
            .get("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), 403);

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "full-lock" }))
            .await;
        assert_eq!(response.status_code(), 403);
    })
    .await;
}

/// A typo must not read as a mode.
/// Silently ignoring it would leave an operator believing the deployment is locked when it is serving.
#[tokio::test]
#[serial]
async fn an_unknown_mode_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let key = owner_key(&ctx, "acme").await;

        let response = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "mode": "readonly" }))
            .await;
        assert_eq!(response.status_code(), 422);
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
#[serial]
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
