use axum::http::StatusCode;
use serde_json::Value;
use yorishiro::App;

use crate::requests::boot_request;

#[tokio::test]
async fn openapi_document_is_mounted_with_enterprise_routes() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/docs/openapi.json").await;
        assert_eq!(response.status_code(), StatusCode::OK);

        let document: Value = response.json();
        assert_eq!(document["info"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(document["components"]["securitySchemes"]["bearer_auth"]["scheme"], "bearer");
        let invalid = request.post("/auth/root/login").json(&serde_json::json!({"email":12,"password":"invalid"})).await;
        invalid.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        assert!(invalid.headers()["content-type"].to_str().unwrap().starts_with("text/plain"));
        assert!(
            document["paths"]["/api/export.jsonl"]["get"]["responses"]["200"]["content"]
                ["application/x-ndjson"].is_object()
        );
        assert!(
            document["paths"]["/api/stripe/webhook"]["post"]["responses"]["400"]["content"]
                ["text/plain; charset=utf-8"].is_object()
        );
        assert_eq!(
            document["paths"]["/api/stripe/webhook"]["post"]["requestBody"]["content"]
                ["application/json"]["schema"]["type"],
            "string"
        );
        assert!(document["paths"]["/api/stripe/webhook"]["post"]["responses"]["500"]["content"]
            .is_null());
        assert_eq!(
            document["paths"]["/api/audit-log"]["get"]["x-yorishiro-required-grants"],
            serde_json::json!(["audit"])
        );
        assert_eq!(
            document["paths"]["/api/members"]["get"]["x-yorishiro-required-actions"],
            serde_json::json!(["tenant_user_list"])
        );
        assert_eq!(
            document["paths"]["/api/entities"]["post"]["security"][0]["bearer_auth"],
            serde_json::json!([])
        );
        assert!(document["components"]["schemas"]["TenantOverview"].is_object());
    })
    .await;
}
