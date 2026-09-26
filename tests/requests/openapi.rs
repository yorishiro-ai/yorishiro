use axum::http::StatusCode;
use serde_json::Value;
use yorishiro::app::App;

use super::boot_request;

#[tokio::test]
async fn openapi_document_is_mounted_with_enterprise_routes() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/docs/openapi.json").await;
        assert_eq!(response.status_code(), StatusCode::OK);

        let document: Value = response.json();
        assert_eq!(document["openapi"], "3.1.0");
        assert!(document["paths"]["/api/entities"].is_object());
        assert!(document["paths"]["/api/marketplace"].is_object());
        assert!(document["paths"]["/mcp"].is_null());
        assert_eq!(
            document["paths"]["/api/export.jsonl"]["get"]["responses"]["200"]["content"]
                ["application/x-ndjson"]["schema"]["type"],
            "string"
        );
        assert_eq!(
            document["paths"]["/api/stripe/webhook"]["post"]["responses"]["400"]["content"]
                ["text/plain"]["schema"]["type"],
            "string"
        );
        assert_eq!(
            document["paths"]["/api/stripe/webhook"]["post"]["parameters"][0]["name"],
            "Stripe-Signature"
        );
        assert_eq!(
            document["paths"]["/api/stripe/webhook"]["post"]["parameters"][0]["required"],
            true
        );
        assert_eq!(
            document["paths"]["/api/stripe/webhook"]["post"]["requestBody"]["content"]
                ["application/json"]["schema"]["type"],
            "string"
        );
        assert!(document["paths"]["/api/stripe/webhook"]["post"]["responses"]["500"]["content"]
            .is_null());
        assert_eq!(
            document["paths"]["/api/entities"]["post"]["x-yorishiro-required-scopes"],
            serde_json::json!(["write"])
        );
        assert_eq!(
            document["paths"]["/api/audit-log"]["get"]["x-yorishiro-required-grants"],
            serde_json::json!(["audit"])
        );
        assert_eq!(
            document["paths"]["/api/members"]["get"]["x-yorishiro-required-roles"],
            serde_json::json!(["tenant_admin"])
        );
        assert_eq!(
            document["paths"]["/api/entities"]["post"]["security"][0]["bearer_auth"],
            serde_json::json!([])
        );
        assert!(document["components"]["schemas"]["TenantOverview"].is_object());
    })
    .await;
}
