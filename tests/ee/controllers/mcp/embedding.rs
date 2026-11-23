use serde_json::json;

use super::*;

#[tokio::test]
async fn embedding_provider_tools_keep_secrets_private_and_match_rest() {
    provider_tools_keep_secrets_private_and_match_rest(
        "embedding",
        "/api/workspace/embedding-key",
        json!({"base_url":"https://embedding.example.invalid/v1","model":"test-embedding","api_key":"test-only-provider-secret","dimensions":32}),
        "test-only-provider-secret",
        ("dimensions", json!(32)),
        ("dimensions", json!(0)),
    )
    .await;
}
