use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;

#[tokio::test]
#[serial_test::serial(process_environment)]
async fn overview_distinguishes_billing_manual_and_active_limits() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    use std::sync::Arc;
    use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
    let variable = crate::env::TestEnv::PlanPolicyEnabled.name();
    let guard = crate::EnvGuard::capture(&[variable]);
    guard.set(variable, "true");
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        yorishiro::models::tenant_tenants::ActiveModel {
            id: Set(tenant.tenant_id),
            max_workspaces: Set(Some(17)),
            ..Default::default()
        }
        .update(&ctx.db)
        .await
        .unwrap();
        for (exp, plan, effective) in [
            (
                chrono::Utc::now().timestamp() + 3600,
                serde_json::json!("pro"),
                5,
            ),
            (
                chrono::Utc::now().timestamp() - 1,
                serde_json::Value::Null,
                17,
            ),
        ] {
            ctx.shared_store
                .insert(Arc::new(LicenceState::licensed(LicenceClaims {
                    sub: "overview".into(),
                    plan: "pro".into(),
                    exp,
                })));
            let response = request
                .get("/api/tenant/overview")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .await;
            response.assert_status(axum::http::StatusCode::OK);
            let body = response.json::<serde_json::Value>();
            assert!(body["plan"].is_null());
            assert_eq!(body["active_plan"], plan);
            assert_eq!(body["max_workspaces"], 17);
            assert_eq!(body["effective_max_workspaces"], effective);
            let workspace = request
                .get(&format!("/api/workspaces/{}", tenant.workspace_id))
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .await;
            workspace.assert_status(axum::http::StatusCode::OK);
            let workspace = workspace.json::<serde_json::Value>();
            assert!(workspace["max_entities"].is_null());
            assert_eq!(
                workspace["effective_max_entities"],
                if plan.is_null() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(50_000)
                }
            );
        }
    })
    .await;
}

/// The freshly bootstrapped owner can read their own tenant's overview: zero usage, no plan (never subscribed), and themselves as the sole member.
#[tokio::test]
async fn tenant_overview_returns_usage_and_members_for_the_owner() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let args = TenantArgs::default();
        let email = args.owner_email.clone();
        let TestTenant { api_key, .. } = TestTenant::create(&request, &ctx, args).await;

        let response = request
            .get("/api/tenant/overview")
            .add_header("Authorization", format!("Bearer {api_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            200,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["plan"], serde_json::Value::Null);
        assert_eq!(body["usage"]["workspace_count"], 1);
        assert_eq!(body["usage"]["member_count"], 1);
        assert_eq!(body["members"].as_array().unwrap().len(), 1);
        assert_eq!(body["members"][0]["email"], email);
        assert_eq!(body["members"][0]["role"], "owner");
    })
    .await;
}

/// No `Authorization` header at all must be rejected, not treated as an anonymous tenant.
#[tokio::test]
async fn tenant_overview_requires_authentication() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/tenant/overview").await;
        assert_eq!(
            response.status_code(),
            401,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}
