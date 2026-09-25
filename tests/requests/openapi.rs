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
        assert!(document["components"]["schemas"]["StripeEvent"].is_object());
    })
    .await;
}
