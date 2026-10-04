use async_trait::async_trait;
use reqwest::{Request, StatusCode, header::HeaderMap};
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use yorishiro::error::YorishiroError;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use yorishiro::ee::services::inference::*;

async fn start_server(response: &str) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("test listener must bind");
    let address = listener.local_addr().expect("test listener has an address");
    let response = response.to_string();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("test request must arrive");
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            let read = socket
                .read(&mut buffer)
                .await
                .expect("test request must be readable");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        socket
            .write_all(response.as_bytes())
            .await
            .expect("test response must be written");
        request
    });
    (format!("http://{address}"), task)
}

struct FakeHttp {
    outcome: Mutex<Option<Result<InferenceHttpResponse, InferenceHttpError>>>,
    request: Mutex<Option<Request>>,
}

#[async_trait]
impl InferenceHttp for FakeHttp {
    async fn send(
        &self,
        _client: &reqwest::Client,
        request: Request,
    ) -> Result<InferenceHttpResponse, InferenceHttpError> {
        *self.request.lock().unwrap() = Some(request);
        self.outcome.lock().unwrap().take().unwrap()
    }
}

fn config() -> InferenceConfig {
    InferenceConfig {
        base_url: "http://inference.test/v1/".into(),
        model: "test-model".into(),
        api_key: "test-api-key".into(),
    }
}

fn response(status: u16, body: &str) -> InferenceHttpResponse {
    InferenceHttpResponse {
        status: StatusCode::from_u16(status).unwrap(),
        headers: HeaderMap::new(),
        body: Box::pin(std::future::ready(Ok(body.as_bytes().to_vec()))),
    }
}

fn response_with_read_flag(
    status: u16,
    body: &str,
    read: Arc<AtomicBool>,
) -> InferenceHttpResponse {
    let body = body.as_bytes().to_vec();
    InferenceHttpResponse {
        status: StatusCode::from_u16(status).unwrap(),
        headers: HeaderMap::new(),
        body: Box::pin(async move {
            read.store(true, Ordering::SeqCst);
            Ok(body)
        }),
    }
}

fn body_error(message: &str) -> InferenceHttpResponse {
    InferenceHttpResponse {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        body: Box::pin(std::future::ready(Err(InferenceHttpError::Response(
            message.into(),
        )))),
    }
}

fn client(
    outcome: Result<InferenceHttpResponse, InferenceHttpError>,
) -> (InferenceClient, Arc<FakeHttp>) {
    let http = Arc::new(FakeHttp {
        outcome: Mutex::new(Some(outcome)),
        request: Mutex::new(None),
    });
    let client = InferenceClient::with_http(config(), reqwest::Client::new(), http.clone());
    (client, http)
}

#[tokio::test]
async fn injected_transport_parses_completion_and_preserves_request_schema() {
    let (client, http) = client(Ok(response(
        200,
        r#"{"choices":[{"message":{"content":"{\"category\":\"news\",\"ignored\":true}"}}]}"#,
    )));

    let proposals = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap();

    assert_eq!(
        proposals,
        serde_json::json!({"category": "news"})
            .as_object()
            .unwrap()
            .clone()
    );
    let request = http.request.lock().unwrap().take().unwrap();
    assert_eq!(
        request.url().as_str(),
        "http://inference.test/v1/chat/completions"
    );
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(
        request.headers()[reqwest::header::AUTHORIZATION],
        "Bearer test-api-key"
    );
    let request_body: Value =
        serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(request_body["model"], "test-model");
    assert_eq!(request_body["temperature"], 0.0);
    assert_eq!(request_body["response_format"]["type"], "json_object");
    assert_eq!(request_body["messages"][0]["role"], "user");
    assert!(
        request_body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Missing fields: category")
    );
}

#[tokio::test]
async fn malformed_completion_is_validation_failed_without_body_or_secret() {
    let (client, _) = client(Ok(response(
        200,
        r#"{"choices":[{"message":{"content":"provider-secret-body"}}]}"#,
    )));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::ValidationFailed { .. }));
    assert!(!format!("{error:?}").contains("provider-secret-body"));
    assert!(!format!("{error:?}").contains("test-api-key"));
}

#[tokio::test]
async fn malformed_provider_response_is_internal() {
    let (client, _) = client(Ok(response(200, r#"{"choices":"invalid"}"#)));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::Internal(_)));
}

#[tokio::test]
async fn malformed_outer_response_preserves_previous_decode_error_text() {
    let (client, _) = client(Ok(response(200, "provider-secret-body")));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::Internal(_)));
    assert_eq!(
        error.to_string(),
        "internal error: error decoding response body"
    );
    assert!(!format!("{error:?}").contains("provider-secret-body"));
    assert!(!format!("{error:?}").contains("test-api-key"));
}

#[tokio::test]
async fn production_reqwest_adapter_sends_request_and_parses_response() {
    let (base_url, task) = start_server(
        "HTTP/1.1 200 OK\r\nContent-Length: 63\r\nConnection: close\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"{\\\"category\\\":\\\"news\\\"}\"}}]}",
    )
    .await;
    let client = InferenceClient::new(InferenceConfig {
        base_url: format!("{base_url}/v1"),
        model: "test-model".into(),
        api_key: "test-api-key".into(),
    });

    let proposals = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .expect("the loopback provider must return a completion");
    let request = task.await.expect("test server must finish");
    let request = String::from_utf8(request).expect("request must be valid HTTP text");

    assert_eq!(proposals["category"], "news");
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-api-key\r\n")
    );
}

#[tokio::test]
async fn production_reqwest_adapter_maps_retry_after_without_reading_error_body() {
    let body = "provider-secret-body";
    let (base_url, task) = start_server(&format!(
        "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 7\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ))
    .await;
    let client = InferenceClient::new(InferenceConfig {
        base_url: format!("{base_url}/v1"),
        model: "test-model".into(),
        api_key: "test-api-key".into(),
    });

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();
    task.await.expect("test server must finish");

    assert!(matches!(
        &error,
        YorishiroError::ProviderBusy { retry_after, .. }
            if *retry_after == Duration::from_secs(7)
    ));
    assert!(!format!("{error:?}").contains(body));
}

#[tokio::test]
async fn production_reqwest_adapter_does_not_follow_redirects() {
    let redirected_listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("redirect target listener must bind");
    let redirected_address = redirected_listener
        .local_addr()
        .expect("redirect target listener has an address");
    let redirected = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(500), redirected_listener.accept())
            .await
            .ok()
            .and_then(Result::ok)
            .is_some()
    });
    let (base_url, task) = start_server(&format!(
        "HTTP/1.1 302 Found\r\nLocation: http://{redirected_address}/v1/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    ))
    .await;
    let client = InferenceClient::new(InferenceConfig {
        base_url: format!("{base_url}/v1"),
        model: "test-model".into(),
        api_key: "test-api-key".into(),
    });

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();
    task.await.expect("redirect server must finish");

    assert!(matches!(error, YorishiroError::ValidationFailed { .. }));
    assert!(!redirected.await.expect("redirect target must finish"));
}

#[tokio::test]
async fn transport_failure_maps_to_provider_unreachable_with_exact_message() {
    let message = "error sending request for url (https://inference.test/v1/chat/completions): connection refused";
    let (client, _) = client(Err(InferenceHttpError::Request(message.into())));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
    assert_eq!(error.code().unwrap().as_str(), "provider_unreachable");
    assert!(error.to_string().contains(message));
}

#[tokio::test]
async fn timeout_maps_to_provider_unreachable_and_redacts_secret() {
    let message = "error sending request for url (https://inference.test/v1/chat/completions): operation timed out";
    let (client, _) = client(Err(InferenceHttpError::Timeout(format!(
        "{message}; key=test-api-key"
    ))));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
    assert!(error.to_string().contains(message));
    assert!(!error.to_string().contains("test-api-key"));
}

#[tokio::test]
async fn provider_busy_status_does_not_read_non_success_body() {
    let read = Arc::new(AtomicBool::new(false));
    let (client, _) = client(Ok(response_with_read_flag(
        503,
        "provider-secret-body",
        read.clone(),
    )));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(
        &error,
        YorishiroError::ProviderBusy { retry_after, .. }
            if *retry_after == Duration::from_secs(5)
    ));
    assert!(!read.load(Ordering::SeqCst));
    assert!(!format!("{error:?}").contains("provider-secret-body"));
}

#[tokio::test]
async fn provider_auth_status_maps_to_validation_without_reading_body() {
    let read = Arc::new(AtomicBool::new(false));
    let (client, _) = client(Ok(response_with_read_flag(
        401,
        "provider-secret-body",
        read.clone(),
    )));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::ValidationFailed { .. }));
    assert!(!read.load(Ordering::SeqCst));
    assert!(!format!("{error:?}").contains("provider-secret-body"));
}

#[tokio::test]
async fn response_body_read_failure_maps_to_internal_and_redacts_api_key() {
    let (client, _) = client(Ok(body_error(
        "provider body read failed after echoing test-api-key",
    )));

    let error = client
        .propose_fields(&serde_json::json!({"title": "A report"}), &["category"])
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::Internal(_)));
    assert!(!error.to_string().contains("test-api-key"));
}

/// Asking for nothing must not produce a request.
/// A workspace whose entities are all complete would otherwise pay for a call whose answer is discarded.
#[tokio::test]
async fn no_missing_fields_makes_no_request() {
    // An unroutable base_url: if a request were made, this would error rather than return empty.
    let client = InferenceClient::new(InferenceConfig {
        base_url: "http://127.0.0.1:1/v1".into(),
        model: "unused".into(),
        api_key: "unused".into(),
    });

    let proposals = client
        .propose_fields(&serde_json::json!({"title": "x"}), &[])
        .await
        .expect("asking for no fields must not call the provider");

    assert!(proposals.is_empty());
}

/// A provider that cannot be reached is reported without leaking the key, since the error carries the caller's own request context.
#[tokio::test]
async fn an_unreachable_provider_is_reported_without_leaking_the_key() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("test listener must bind");
    let address = listener.local_addr().expect("test listener has an address");
    drop(listener);
    let client = InferenceClient::new(InferenceConfig {
        base_url: format!("http://{address}/v1"),
        model: "unused".into(),
        api_key: "test-api-key".into(),
    });

    let error = client
        .propose_fields(&serde_json::json!({"title": "x"}), &["category"])
        .await
        .expect_err("an unroutable provider must fail");

    let rendered = format!("{error:?}");
    assert!(!rendered.contains("test-api-key"));
}
