use std::fmt::Write as _;

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
        let rendered = operation_summary(&document);
        if std::env::var_os("UPDATE_FIXTURES").is_some_and(|value| value == "1") {
            std::fs::write("tests/fixtures/openapi-operations.txt", &rendered).unwrap();
        } else {
            assert_eq!(
                include_str!("../../../fixtures/openapi-operations.txt"),
                rendered,
                "Regenerate with UPDATE_FIXTURES=1 cargo test --features enterprise openapi_document_is_mounted_with_enterprise_routes"
            );
        }
        for (path, method) in [
            ("/api/entities", "post"),
            ("/api/search", "get"),
            ("/api/schema-forks", "post"),
        ] {
            let operation = &document["paths"][path][method];
            let headers = operation["parameters"].as_array().unwrap();
            assert_eq!(
                headers.iter().filter(|parameter| {
                    parameter["in"] == "header" && parameter["name"] == "X-Workspace-Id"
                }).count(),
                1,
                "{method} {path}"
            );
        }
        assert!(document["paths"]["/api/schema-forks"]["post"]["responses"]["422"].is_object());
        assert_eq!(document["openapi"], "3.1.0");
        assert!(document["paths"]["/auth/login"].is_null());
        assert!(document["paths"]["/auth/root/login"]["post"].is_object());
        assert!(document["paths"]["/auth/tenant/login"]["post"].is_object());
        let invalid = request.post("/auth/root/login").json(&serde_json::json!({"email":12,"password":"invalid"})).await;
        invalid.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        assert!(invalid.headers()["content-type"].to_str().unwrap().starts_with("text/plain"));
        assert!(document["paths"]["/api/entities"].is_object());
        assert!(document["paths"]["/api/entities/{id}"]["put"]["responses"]["409"].is_object());
        assert!(document["paths"]["/api/schemas/{schema_id}"]["delete"]["responses"]["401"].is_object());
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
            document["paths"]["/api/members"]["get"]["x-yorishiro-required-actions"],
            serde_json::json!(["tenant_user_list"])
        );
        for method in ["put", "delete"] {
            let operation = &document["paths"]["/api/workspaces/{id}/members/{user_id}"][method];
            assert_eq!(operation["x-yorishiro-required-actions"], serde_json::json!(["workspace_update"]));
            assert!(operation.get("x-yorishiro-required-roles").is_none());
            for status in ["204", "401", "403", "404"] {
                assert!(operation["responses"][status].is_object());
            }
        }
        assert_eq!(
            document["paths"]["/api/entities"]["post"]["security"][0]["bearer_auth"],
            serde_json::json!([])
        );
        assert!(document["components"]["schemas"]["TenantOverview"].is_object());
    })
    .await;
}

fn operation_summary(document: &Value) -> String {
    let mut lines = Vec::new();
    for (path, methods) in document["paths"].as_object().unwrap() {
        for (method, operation) in methods.as_object().unwrap() {
            let mut parameters = operation["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|parameter| {
                    let location = parameter["in"].as_str().unwrap();
                    let name = parameter["name"].as_str().unwrap();
                    let optional = if parameter["required"].as_bool() == Some(true) {
                        ""
                    } else {
                        "?"
                    };
                    format!("{location}:{name}{optional}")
                })
                .collect::<Vec<_>>();
            parameters.sort_unstable();
            let mut statuses = operation["responses"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            statuses.sort_unstable();
            lines.push(format!(
                "{} {path} [{}] -> {}",
                method.to_ascii_uppercase(),
                parameters.join(", "),
                statuses.join(", ")
            ));
        }
    }
    lines.sort_unstable();
    let mut summary = String::new();
    for line in lines {
        writeln!(summary, "{line}").unwrap();
    }
    summary
}
