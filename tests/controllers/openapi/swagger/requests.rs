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
        for path in document["paths"].as_object().unwrap().keys() {
            assert!(
                !path.split(['{', '}']).skip(1).step_by(2).any(|name| matches!(name, "id" | "job_id")),
                "{path}"
            );
        }
        assert_eq!(document["info"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(document["openapi"], "3.1.0");
        let policy = &document["components"]["schemas"]["IamPolicyDocument"];
        assert_eq!(policy["properties"]["Version"]["const"], "2012-10-17");
        assert_eq!(policy["x-iam-actions"]["iam:AttachGroupPolicy"], "permissions_management");
        assert_eq!(policy["x-iam-actions"]["entity:ListEntities"], "list");
        assert_eq!(
            policy["x-iam-actions"].as_object().unwrap().len(),
            yorishiro::models::iam::policy::Action::ALL.len()
        );
        let statement = &document["components"]["schemas"]["IamPolicyStatement"];
        assert_eq!(statement["oneOf"][0]["required"][0], "Action");
        assert_eq!(statement["oneOf"][1]["required"][0], "NotAction");
        assert_eq!(document["components"]["securitySchemes"]["bearer_auth"]["scheme"], "bearer");
        let invalid = request.post("/auth/root/login").json(&serde_json::json!({"email":12,"password":"invalid"})).await;
        invalid.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        assert!(invalid.headers()["content-type"].to_str().unwrap().starts_with("text/plain"));
        assert!(
            document["paths"]["/api/export"]["get"]["responses"]["200"]["content"]
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
        assert_eq!(
            document["paths"]["/api/stripe/webhook"]["post"]["x-yorishiro-licence-required"],
            true
        );
        assert!(document["paths"]["/api/workspace/llm-provider"]["put"]
            ["x-yorishiro-licence-required"].is_null());
        assert!(document["paths"]["/api/stripe/webhook"]["post"]["responses"]["500"]["content"]
            .is_null());
        assert_eq!(
            document["paths"]["/api/audit-log"]["get"]["operationId"],
            "ListWorkspaceAuditEvents"
        );
        assert_eq!(
            document["paths"]["/api/users"]["get"]["operationId"],
            "ListUsers"
        );
        assert!(document["paths"]["/api/users"]["get"]["x-yorishiro-required-actions"].is_null());
        for (path, id) in [
            ("/api/policies/{policy_id}/versions/{version_id}:set-default", "SetDefaultPolicyVersion"),
            ("/api/schemas/{schema_id}:archive", "ArchiveSchema"),
            ("/api/migration-jobs/{migration_job_id}:undo", "UndoMigrationJob"),
        ] {
            assert_eq!(document["paths"][path]["post"]["operationId"], id);
            assert!(document["paths"][path.split_once(':').unwrap().0]["post"].is_null());
        }
        assert_eq!(document["paths"]["/api/schemas/{schema_id}"]["get"]["operationId"], "GetSchema");
        assert_eq!(document["paths"]["/api/schemas/{schema_id}"]["delete"]["operationId"], "DeleteSchema");
        assert_eq!(
            document["paths"]["/api/entities"]["post"]["security"][0]["bearer_auth"],
            serde_json::json!([])
        );
        assert!(document["paths"].get("/api/tenant/overview").is_none());
    })
    .await;
}
