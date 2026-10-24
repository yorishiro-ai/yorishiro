use std::time::Duration;

use axum::{
    Json, Router,
    body::Bytes,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use yorishiro::{
    YorishiroError,
    services::embedding::{EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider},
};

use crate::http_stub::HttpStub;

fn config(base_url: String) -> OpenAiCompatibleConfig {
    OpenAiCompatibleConfig {
        base_url,
        api_key: "test-api-key".into(),
        model: "test-model".into(),
        dimensions: 2,
        send_dimensions_param: true,
    }
}

async fn provider(
    status: u16,
    body: &str,
    retry_after: Option<&str>,
) -> (OpenAiCompatibleProvider, HttpStub) {
    let body = body.to_owned();
    let mut headers = HeaderMap::new();
    if let Some(after) = retry_after {
        headers.insert(reqwest::header::RETRY_AFTER, after.parse().unwrap());
    }
    let app = Router::new().route(
        "/embeddings",
        post(move || async move { (StatusCode::from_u16(status).unwrap(), headers, body) }),
    );
    let server = HttpStub::start(app).await;
    (
        OpenAiCompatibleProvider::new(config(server.url.clone())),
        server,
    )
}

#[tokio::test]
async fn production_transport_preserves_request_and_provider_order() {
    let (sent, received) = tokio::sync::oneshot::channel();
    let sent = std::sync::Arc::new(tokio::sync::Mutex::new(Some(sent)));
    let app = Router::new().route(
        "/embeddings",
        post(move |headers: HeaderMap, body: Bytes| async move {
            sent.lock()
                .await
                .take()
                .unwrap()
                .send((headers, body))
                .unwrap();
            Json(serde_json::json!({"data":[{"index":1,"embedding":[3.0,4.0]},{"index":0,"embedding":[1.0,2.0]}]}))
        }),
    );
    let server = HttpStub::start(app).await;
    let vectors = OpenAiCompatibleProvider::new(config(server.url.clone()))
        .embed_batch(&["first", "second"])
        .await
        .unwrap();
    assert_eq!(vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    let (headers, body) = received.await.unwrap();
    assert_eq!(
        headers[reqwest::header::AUTHORIZATION],
        "Bearer test-api-key"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"model":"test-model","input":["first","second"],"dimensions":2})
    );
}

#[tokio::test]
async fn production_status_mapping_preserves_retry_bound_and_redacts_body() {
    for (status, header, seconds) in [
        (429, None, 5),
        (503, Some("13"), 13),
        (503, Some("999"), 60),
        (429, Some("invalid"), 5),
        (500, None, 0),
    ] {
        let (provider, _server) =
            provider(status, "provider-secret-body test-api-key", header).await;
        let error = provider.embed_batch(&["text"]).await.unwrap_err();
        if seconds > 0 {
            assert!(
                matches!(&error, YorishiroError::ProviderBusy { retry_after, .. } if *retry_after == Duration::from_secs(seconds))
            );
        } else {
            assert!(matches!(&error, YorishiroError::Internal(_)));
        }
        assert!(!format!("{error:?}").contains("provider-secret-body"));
        assert!(!format!("{error:?}").contains("test-api-key"));
    }
}

#[tokio::test]
async fn production_response_validation_rejects_invalid_json_count_and_width() {
    for (body, inputs, diagnostic) in [
        ("provider-secret-body test-api-key", vec!["text"], None),
        (
            r#"{"data":[{"index":1,"embedding":[1.0,2.0]}]}"#,
            vec!["text"],
            Some("invalid vector indices"),
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1.0,2.0]},{"index":0,"embedding":[3.0,4.0]}]}"#,
            vec!["first", "second"],
            Some("invalid vector indices"),
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1.0,2.0]},{"embedding":[3.0,4.0]}]}"#,
            vec!["first", "second"],
            None,
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1.0,2.0]}]}"#,
            vec!["first", "second"],
            Some("1 vectors for 2 inputs"),
        ),
        (
            r#"{"data":[{"index":0,"embedding":[1.0]}]}"#,
            vec!["text"],
            Some("length 1 but expected 2"),
        ),
    ] {
        let (provider, _server) = provider(200, body, None).await;
        let error = provider.embed_batch(&inputs).await.unwrap_err();
        assert!(matches!(&error, YorishiroError::Internal(_)));
        if let Some(diagnostic) = diagnostic {
            assert!(error.to_string().contains(diagnostic));
        }
        assert!(!format!("{error:?}").contains("provider-secret-body"));
        assert!(!format!("{error:?}").contains("test-api-key"));
    }
}

#[tokio::test]
async fn real_request_timeout_and_connection_failure_are_unreachable() {
    let server = HttpStub::stalled().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let provider = OpenAiCompatibleProvider::with_client(config(server.url.clone()), client);
    let error = provider.embed_batch(&["text"]).await.unwrap_err();
    assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
    assert!(!error.to_string().contains("test-api-key"));
    drop(server);
    let error = provider.embed_batch(&["text"]).await.unwrap_err();
    assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
}

#[tokio::test]
async fn truncated_success_body_is_internal() {
    let server = HttpStub::raw("HTTP/1.1 200 OK\r\nContent-Length: 99999\r\nConnection: close\r\n\r\n{\"secret\":\"test-api-key\"}".into()).await;
    let error = OpenAiCompatibleProvider::new(config(server.url.clone()))
        .embed_batch(&["text"])
        .await
        .unwrap_err();
    assert!(matches!(&error, YorishiroError::Internal(_)));
    assert!(!error.to_string().contains("test-api-key"));
}
