use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;

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
