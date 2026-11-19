use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;

#[tokio::test]
async fn overview_distinguishes_billing_manual_and_active_limits() {
    use sea_orm::ActiveModelTrait;
    use std::sync::Arc;
    use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
    let _config = crate::requests::ConfigOverride::set(&[(
        "initializers.plan_policy_enabled",
        "true".into(),
    )]);
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        crate::requests::fixtures::set_workspace_cap(&ctx, tenant.tenant_id, 17).await;
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
                .add_header("Authorization", format!("Bearer {tenant_key}"))
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

#[tokio::test]
async fn tenant_overview_returns_usage_and_members_for_the_owner() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let api_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        request
            .get("/api/tenant/overview")
            .await
            .assert_status(axum::http::StatusCode::UNAUTHORIZED);
        request
            .get("/api/tenant/overview")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(axum::http::StatusCode::FORBIDDEN);

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
        assert!(body["members"].as_array().unwrap().is_empty());
    })
    .await;
}
