use std::time::Duration;

use axum::{Json, Router, body::Bytes, http::HeaderMap, routing::post};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use yorishiro::{
    YorishiroError,
    edition::ee::services::inference::{InferenceClient, InferenceConfig},
};

fn config(base_url: String) -> InferenceConfig {
    InferenceConfig {
        base_url,
        model: "test-model".into(),
        api_key: "test-api-key".into(),
    }
}

async fn start_server(response: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    (format!("http://{address}/v1"), task)
}

fn response(status: u16, body: &str, extra_headers: &str) -> String {
    format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn production_transport_parses_completion_and_preserves_request_schema() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let (sent, received) = tokio::sync::oneshot::channel();
    let sent = std::sync::Arc::new(tokio::sync::Mutex::new(Some(sent)));
    let app = Router::new().route("/v1/chat/completions", post(move |headers: HeaderMap, body: Bytes| async move {
        sent.lock().await.take().unwrap().send((headers, body)).unwrap();
        Json(serde_json::json!({"choices":[{"message":{"content":"{\"category\":\"news\",\"ignored\":true}"}}]}))
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let proposals = InferenceClient::new(config(format!("http://{address}/v1/")))
        .propose_fields(&serde_json::json!({"title":"A report"}), &["category"])
        .await
        .unwrap();
    assert_eq!(
        proposals,
        serde_json::json!({"category":"news"})
            .as_object()
            .unwrap()
            .clone()
    );
    let (headers, body) = received.await.unwrap();
    assert_eq!(
        headers[reqwest::header::AUTHORIZATION],
        "Bearer test-api-key"
    );
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["response_format"]["type"], "json_object");
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(
        body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Missing fields: category")
    );
    server.abort();
}

#[tokio::test]
async fn malformed_completions_and_outer_responses_remain_safe() {
    for (body, validation) in [
        (
            r#"{"choices":[{"message":{"content":"provider-secret-body test-api-key"}}]}"#,
            true,
        ),
        (r#"{"choices":"invalid"}"#, false),
        ("provider-secret-body test-api-key", false),
    ] {
        let (url, server) = start_server(response(200, body, "")).await;
        let error = InferenceClient::new(config(url))
            .propose_fields(&serde_json::json!({}), &["category"])
            .await
            .unwrap_err();
        server.await.unwrap();
        if validation {
            assert!(matches!(&error, YorishiroError::ValidationFailed { .. }));
        } else {
            assert!(matches!(&error, YorishiroError::Internal(_)));
            assert_eq!(
                error.to_string(),
                "internal error: error decoding response body"
            );
        }
        assert!(!format!("{error:?}").contains("provider-secret-body"));
        assert!(!format!("{error:?}").contains("test-api-key"));
    }
}

#[tokio::test]
async fn non_success_responses_do_not_consume_even_truncated_bodies() {
    for (status, headers, seconds) in [
        (503, "Retry-After: 7\r\n", 7),
        (429, "", 5),
        (503, "Retry-After: 999\r\n", 60),
        (401, "", 0),
    ] {
        // A truncated success body is a read error, but a failure status must be handled before reading it.
        let (url, server) = start_server(format!("HTTP/1.1 {status} Test\r\nContent-Length: 99999\r\nConnection: close\r\n{headers}\r\nprovider-secret-body test-api-key")).await;
        let error = InferenceClient::new(config(url))
            .propose_fields(&serde_json::json!({}), &["category"])
            .await
            .unwrap_err();
        server.await.unwrap();
        if seconds > 0 {
            assert!(
                matches!(&error, YorishiroError::ProviderBusy { retry_after, .. } if *retry_after == Duration::from_secs(seconds))
            );
        } else {
            assert!(matches!(&error, YorishiroError::ValidationFailed { .. }));
        }
        assert!(!format!("{error:?}").contains("provider-secret-body"));
        assert!(!format!("{error:?}").contains("test-api-key"));
    }
}

#[tokio::test]
async fn production_client_does_not_follow_redirects() {
    let target = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let target_address = target.local_addr().unwrap();
    let (url, server) = start_server(response(
        302,
        "",
        &format!("Location: http://{target_address}/v1/chat/completions\r\n"),
    ))
    .await;
    let error = InferenceClient::new(config(url))
        .propose_fields(&serde_json::json!({}), &["category"])
        .await
        .unwrap_err();
    assert!(matches!(error, YorishiroError::ValidationFailed { .. }));
    server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), target.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn timeout_and_connection_failure_are_unreachable_and_empty_fields_skip_http() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let client = InferenceClient::with_client(
        config(format!("http://{address}")),
        reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap(),
    );
    assert!(
        client
            .propose_fields(&serde_json::json!({}), &[])
            .await
            .unwrap()
            .is_empty()
    );
    let error = client
        .propose_fields(&serde_json::json!({}), &["category"])
        .await
        .unwrap_err();
    assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
    assert!(!error.to_string().contains("test-api-key"));
    drop(listener);
    let error = client
        .propose_fields(&serde_json::json!({}), &["category"])
        .await
        .unwrap_err();
    assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
    assert!(!error.to_string().contains("test-api-key"));
}

#[tokio::test]
async fn truncated_success_body_is_internal_and_redacted() {
    let (url, server) = start_server("HTTP/1.1 200 OK\r\nContent-Length: 99999\r\nConnection: close\r\n\r\nprovider-secret-body test-api-key".into()).await;
    let error = InferenceClient::new(config(url))
        .propose_fields(&serde_json::json!({}), &["category"])
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(matches!(&error, YorishiroError::Internal(_)));
    assert!(!format!("{error:?}").contains("test-api-key"));
    assert!(!format!("{error:?}").contains("provider-secret-body"));
}
