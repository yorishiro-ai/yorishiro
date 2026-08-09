use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::YorishiroError;
use crate::services::embedding::EmbeddingProvider;
use crate::services::embedding::openai::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

fn provider(base_url: String) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url,
        api_key: "test-key".into(),
        model: "text-embedding-3-small".into(),
        dimensions: 3,
        send_dimensions_param: true,
    })
}

#[tokio::test]
async fn embeds_a_batch_of_texts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "embedding": [0.1, 0.2, 0.3] },
                { "embedding": [0.4, 0.5, 0.6] }
            ]
        })))
        .mount(&server)
        .await;

    let provider = provider(server.uri());
    let vectors = provider.embed_batch(&["hello", "world"]).await.unwrap();

    assert_eq!(vectors, vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]]);
}

#[tokio::test]
async fn embed_delegates_to_embed_batch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "embedding": [1.0, 2.0, 3.0] }]
        })))
        .mount(&server)
        .await;

    let provider = provider(server.uri());
    let vector = provider.embed("hello").await.unwrap();
    assert_eq!(vector, vec![1.0, 2.0, 3.0]);
}

#[tokio::test]
async fn empty_batch_short_circuits_without_a_request() {
    let server = MockServer::start().await;
    // `expect(0)` means an actual request would panic when the mock server
    // is dropped, catching a regression here.
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let provider = provider(server.uri());
    let vectors = provider.embed_batch(&[]).await.unwrap();
    assert!(vectors.is_empty());
}

#[tokio::test]
async fn omits_dimensions_param_when_disabled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .and(wiremock::matchers::body_json(json!({
            "model": "text-embedding-3-small",
            "input": ["hello"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "embedding": [0.1, 0.2, 0.3] }]
        })))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.uri(),
        api_key: "test-key".into(),
        model: "text-embedding-3-small".into(),
        dimensions: 3,
        send_dimensions_param: false,
    });

    let vectors = provider.embed_batch(&["hello"]).await.unwrap();
    assert_eq!(vectors, vec![vec![0.1, 0.2, 0.3]]);
}

#[tokio::test]
async fn rejects_mismatched_vector_dimensions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "embedding": [0.1, 0.2] }]
        })))
        .mount(&server)
        .await;

    let provider = provider(server.uri());
    let err = provider.embed_batch(&["hello"]).await.unwrap_err();
    assert!(matches!(err, YorishiroError::Internal(_)));
}

#[tokio::test]
async fn rejects_non_success_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let provider = provider(server.uri());
    let err = provider.embed_batch(&["hello"]).await.unwrap_err();
    assert!(matches!(err, YorishiroError::Internal(_)));
}
