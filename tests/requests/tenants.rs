#![cfg(feature = "enterprise")]

use axum::http::StatusCode;
use serde_json::{Value, json};
use yorishiro::App;

use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn owner_can_inspect_and_confirm_disablement_without_deleting_data() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        request
            .get("/api/tenants/current/retirement")
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let before = request
            .get("/api/tenants/current/retirement")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .await;
        assert_eq!(before.status_code(), StatusCode::OK);
        let before = before.json::<Value>();
        assert_eq!(before["tenant_id"], target.tenant_id.to_string());
        assert_eq!(before["identity_policy"], "retain_all_user_accounts");
        assert!(!before.to_string().contains(&other.tenant_id.to_string()));
        assert!(!before.to_string().contains(&target.api_key));
        request
            .post("/api/tenants/current/disable")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .json(&json!({"confirmed_tenant_id":other.tenant_id}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        request
            .post("/api/tenants/current/disable")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .json(&json!({}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        request
            .post("/api/tenants/current/disable")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .json(&json!({"confirmed_tenant_id":target.tenant_id}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get("/api/tenants/current/retirement")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        request
            .post("/api/tenants/current/disable")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .json(&json!({"confirmed_tenant_id":target.tenant_id}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let after =
            yorishiro::models::tenant_tenants::retirement::inventory(&ctx.db, target.tenant_id)
                .await
                .unwrap();
        assert_eq!(after.api_keys, 0);
        assert_eq!(
            serde_json::to_value(after.workspace_ids).unwrap(),
            before["workspace_ids"]
        );
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await
            .assert_status(StatusCode::OK);
    })
    .await;
}
