use axum::http::StatusCode;
use serde_json::Value;
use yorishiro::App;

use super::boot_request;

#[tokio::test]
async fn openapi_document_is_mounted_with_enterprise_routes() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/docs/openapi.json").await;
        assert_eq!(response.status_code(), StatusCode::OK);

        let document: Value = response.json();
        assert_eq!(document["openapi"], "3.1.0");
        let login_422 = &document["paths"]["/auth/login"]["post"]["responses"]["422"]["content"];
        assert_eq!(login_422["application/json"]["schema"]["$ref"], "#/components/schemas/LoginRecoveryResponse");
        assert_eq!(login_422["text/plain"]["schema"]["type"], "string");
        let invalid = request.post("/auth/login").json(&serde_json::json!({"email":12,"password":"invalid"})).await;
        invalid.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        assert!(invalid.headers()["content-type"].to_str().unwrap().starts_with("text/plain"));
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
        for method in ["put", "delete"] {
            let operation = &document["paths"]["/api/workspaces/{id}/members/{user_id}"][method];
            assert_eq!(operation["x-yorishiro-required-roles"], serde_json::json!(["workspace_admin"]));
            for status in ["204", "401", "403", "404"] {
                assert!(operation["responses"][status].is_object());
            }
        }
        assert_eq!(
            document["paths"]["/api/entities"]["post"]["security"][0]["bearer_auth"],
            serde_json::json!([])
        );
        assert!(document["components"]["schemas"]["TenantOverview"].is_object());
        let schemas = &document["components"]["schemas"];
        assert_eq!(
            schemas["InferenceProposalResponse"]["properties"]["status"]["$ref"],
            "#/components/schemas/ProposalStatus"
        );
        assert_eq!(
            schemas["ProposalStatus"]["enum"],
            serde_json::json!(["pending", "confirmed", "rejected", "discarded", "stale", "invalid"])
        );
        assert_eq!(
            schemas["EmbeddingKeyRequest"]["properties"]["send_dimensions_param"]["default"],
            false
        );
        assert_eq!(
            schemas["PublishVersionRequest"]["properties"]["status"]["default"],
            "draft"
        );
        assert_eq!(
            schemas["TemplateVersionRecord"]["properties"]["created_at"]["format"],
            "date-time"
        );
        assert_eq!(
            schemas["MergeResponse"]["properties"]["diff"]["$ref"],
            "#/components/schemas/VersioningDiff"
        );
        assert_eq!(
            document["components"]["schemas"]["InferFillStatus"]["enum"],
            serde_json::json!(["queued"])
        );
        assert_eq!(
            document["components"]["schemas"]["InferenceJobStatus"]["enum"],
            serde_json::json!(["queued", "running", "completed", "failed"])
        );
        assert_eq!(
            document["components"]["schemas"]["TemplateVersionStatus"]["enum"],
            serde_json::json!(["draft", "pre", "stable"])
        );
        assert_eq!(
            document["components"]["schemas"]["TemplateVisibility"]["enum"],
            serde_json::json!(["tenant", "community"])
        );
    })
    .await;
}
