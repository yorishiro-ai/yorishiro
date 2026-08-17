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
    // `expect(0)` means an actual request would panic when the mock server is dropped, catching a regression here.
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

/// A rate-limited provider is telling the caller to come back, not that the request is wrong.
/// Reported as its own variant so the embedding sync waits instead of dropping the work: an embedding lost to a 429 leaves the entity out of search until someone runs a resync.
#[tokio::test]
async fn a_rate_limited_provider_is_reported_as_busy_with_its_own_delay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "7")
                .set_body_string("slow down"),
        )
        .mount(&server)
        .await;

    let err = provider(server.uri()).embed("anything").await.unwrap_err();

    match err {
        YorishiroError::ProviderBusy { retry_after, .. } => {
            assert_eq!(
                retry_after.as_secs(),
                7,
                "the provider's own window is honoured"
            );
        }
        other => panic!("expected ProviderBusy, got {other:?}"),
    }
}

/// Without the header there is still no reason to hammer, so a default stands in.
#[tokio::test]
async fn a_busy_provider_without_a_header_still_gets_a_delay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let err = provider(server.uri()).embed("anything").await.unwrap_err();

    match err {
        YorishiroError::ProviderBusy { retry_after, .. } => {
            assert!(retry_after.as_secs() > 0);
        }
        other => panic!("expected ProviderBusy, got {other:?}"),
    }
}

/// An hour-long Retry-After is capped.
/// The work is recoverable by a resync; a task sleeping for an hour is not something to hold the process open for.
#[tokio::test]
async fn an_extravagant_retry_after_is_capped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3600"))
        .mount(&server)
        .await;

    let err = provider(server.uri()).embed("anything").await.unwrap_err();

    match err {
        YorishiroError::ProviderBusy { retry_after, .. } => {
            assert!(
                retry_after.as_secs() <= 60,
                "capped, got {}s",
                retry_after.as_secs()
            );
        }
        other => panic!("expected ProviderBusy, got {other:?}"),
    }
}

/// A request the provider will never accept stays an internal error.
/// Retrying a 400 would spend the budget on something that cannot succeed.
#[tokio::test]
async fn a_rejected_request_is_not_treated_as_busy() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad input"))
        .mount(&server)
        .await;

    let err = provider(server.uri()).embed("anything").await.unwrap_err();

    assert!(
        matches!(err, YorishiroError::Internal(_)),
        "a 400 is not a reason to come back: {err:?}"
    );
}

/// A provider that is not listening at all.
/// This is the case an operator can fix, and the only way they can is if the response says which endpoint failed, so the error carries the base URL rather than collapsing into `internal server error`.
///
/// Port 1 on the loopback address: privileged, so an unprivileged test process could not have bound it, and nothing in the test environment does.
/// The connection is refused immediately rather than hanging until the 30s timeout.
/// Dropping a `MockServer` is not equivalent: the port can still answer, and this test then fails on a 404 from whatever picked it up, which looks like the bug it is meant to catch.
#[tokio::test]
async fn an_unreachable_provider_is_not_an_internal_error() {
    let uri = "http://127.0.0.1:1".to_string();

    let err = provider(uri.clone()).embed("anything").await.unwrap_err();

    match err {
        YorishiroError::ProviderUnreachable { url, .. } => {
            assert_eq!(url, uri, "the error must name the endpoint that failed");
        }
        other => panic!("expected ProviderUnreachable, got {other:?}"),
    }
}
