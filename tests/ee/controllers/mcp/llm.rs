use serde_json::json;

use super::*;

#[tokio::test]
async fn llm_provider_tools_keep_secrets_private_and_match_rest() {
    provider_tools_keep_secrets_private_and_match_rest(
        "llm",
        "/api/workspace/llm-provider",
        json!({"base_url":"https://llm.example.invalid/v1","model":"test-llm","api_key":"test-only-llm-secret"}),
        "test-only-llm-secret",
        ("model", json!("test-llm")),
        ("api_key", json!("")),
    )
    .await;
}
