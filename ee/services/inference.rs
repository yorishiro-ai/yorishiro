//! Calling an LLM to propose values for fields an entity is missing.
//! Errors use `ProviderUnreachable`/`ProviderBusy` (`crate::services::embedding::openai`), since a request that never reached the provider or one that answered "come back later" are both operator-actionable, not internal server errors.
//!
//! The one place this crate makes an outbound LLM call.
//! Everything else that reaches a model goes through `crate::services::embedding`, which produces vectors rather than text.
//!
//! The credentials belong to a workspace, not to the deployment: this product does not pay for inference, so a workspace that wants inferred values brings its own key.
//! A workspace with no key configured gets a `ValidationFailed` rather than a silent fall back to `default` values: a caller who asked for inference and received defaults would have no way to tell that nothing was inferred.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Request, StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::YorishiroError;

/// Longer than the embedding provider's 30s: a chat completion over several fields is a slower call than embedding one string, and the work is already asynchronous behind a job id, so a caller is not sitting on this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// An OpenAI-compatible chat-completions endpoint, configured per workspace.
///
/// The same shape `crate::services::embedding::OpenAiCompatibleConfig` takes, so a deployment pointing at Ollama or LM Studio configures both the same way.
#[derive(Clone)]
pub struct InferenceConfig {
    /// Example: `https://api.openai.com/v1` (a trailing `/` is optional).
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

pub struct InferenceClient {
    http: Arc<dyn InferenceHttp>,
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl InferenceClient {
    pub fn new(config: InferenceConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // No redirects: a workspace sets `base_url` freely, and this request carries that workspace's bearer key, so following a redirect would re-send it to a host nobody configured.
            // This does not make the destination safe: `base_url` itself is still unrestricted, which is a policy question about what a tenant may point the server at (see `ee/docs/api.md`).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client configuration is static and always valid");

        Self::with_http(config, client, Arc::new(ReqwestInferenceHttp))
    }

    fn with_http(
        config: InferenceConfig,
        client: reqwest::Client,
        http: Arc<dyn InferenceHttp>,
    ) -> Self {
        Self {
            http,
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            model: config.model,
            api_key: config.api_key,
        }
    }

    /// Asks the model for values for `missing_fields`, given what the entity already holds.
    ///
    /// Returns only the fields the model answered with, and only those that were asked for: a model that invents a key would otherwise write a field the schema does not define.
    /// A field the model declines to guess is absent from the result rather than null, so the caller can tell "no proposal" from "proposed nothing".
    pub async fn propose_fields(
        &self,
        entity_data: &Value,
        missing_fields: &[&str],
    ) -> Result<serde_json::Map<String, Value>, YorishiroError> {
        if missing_fields.is_empty() {
            return Ok(serde_json::Map::new());
        }

        let prompt = format!(
            "Given this record, propose values for the listed missing fields.\n\
             Answer with a JSON object containing only those field names. Omit any field you \
             cannot infer from the record: do not guess blindly, and do not invent field \
             names that are not listed.\n\n\
             Record:\n{}\n\nMissing fields: {}",
            serde_json::to_string_pretty(entity_data).unwrap_or_else(|_| "{}".into()),
            missing_fields.join(", "),
        );

        let request = ChatRequest {
            model: &self.model,
            messages: vec![ChatMessage {
                role: "user",
                content: &prompt,
            }],
            // Deterministic: the same record should not produce a different proposal each run, or a caller comparing two runs cannot tell a model's uncertainty from a change in the data.
            temperature: 0.0,
            response_format: ResponseFormat {
                kind: "json_object",
            },
        };

        let request = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request)
            .build()
            .map_err(|err| YorishiroError::ProviderUnreachable {
                url: self.base_url.clone(),
                message: err.to_string(),
            })?;
        let response = self
            .http
            .send(&self.client, request)
            .await
            .map_err(|err| match err {
                InferenceHttpError::Timeout(_) | InferenceHttpError::Request(_) => {
                    YorishiroError::ProviderUnreachable {
                        url: self.base_url.clone(),
                        message: err.message(&self.api_key),
                    }
                }
                InferenceHttpError::Response(_) => {
                    YorishiroError::Internal(anyhow::anyhow!(err.message(&self.api_key)))
                }
            })?;

        let status = response.status;
        if !status.is_success() {
            if let Some(after) = retry_after(status.as_u16(), &response.headers) {
                return Err(YorishiroError::ProviderBusy {
                    message: format!("the configured inference provider returned {status}"),
                    retry_after: after,
                });
            }
            // The body may quote the key back or carry provider-side detail; neither belongs in an error a tenant reads.
            // The status is what tells an operator whether to fix the key (401), the model name (404), or wait (429/503, handled above).
            return Err(YorishiroError::ValidationFailed {
                message: format!("the configured inference provider returned {status}"),
                details: vec![],
                hint: "check the workspace's LLM base_url, model and api_key".into(),
            });
        }

        let body = response
            .body
            .await
            .map_err(|err| YorishiroError::Internal(anyhow::anyhow!(err.message(&self.api_key))))?;
        let body: ChatResponse = serde_json::from_slice(&body).map_err(|_| {
            YorishiroError::Internal(anyhow::anyhow!("error decoding response body"))
        })?;
        let content = body
            .choices
            .first()
            .map(|c| c.message.content.as_str())
            .unwrap_or("{}");

        let parsed: Value = serde_json::from_str(content).unwrap_or(Value::Null);
        let Some(object) = parsed.as_object() else {
            return Err(YorishiroError::ValidationFailed {
                message: "the inference provider did not answer with a JSON object".into(),
                details: vec![],
                hint: "the model may not support the json_object response format".into(),
            });
        };

        Ok(object
            .iter()
            .filter(|(key, _)| missing_fields.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }
}

#[async_trait]
trait InferenceHttp: Send + Sync {
    async fn send(
        &self,
        client: &reqwest::Client,
        request: Request,
    ) -> Result<InferenceHttpResponse, InferenceHttpError>;
}

struct ReqwestInferenceHttp;

#[async_trait]
impl InferenceHttp for ReqwestInferenceHttp {
    async fn send(
        &self,
        client: &reqwest::Client,
        request: Request,
    ) -> Result<InferenceHttpResponse, InferenceHttpError> {
        let response = client
            .execute(request)
            .await
            .map_err(InferenceHttpError::from_request)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes();
        Ok(InferenceHttpResponse {
            status,
            headers,
            body: Box::pin(async move {
                body.await
                    .map(|body| body.to_vec())
                    .map_err(|error| InferenceHttpError::Response(error.to_string()))
            }),
        })
    }
}

struct InferenceHttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Pin<Box<dyn Future<Output = Result<Vec<u8>, InferenceHttpError>> + Send>>,
}

enum InferenceHttpError {
    Timeout(String),
    Request(String),
    Response(String),
}

impl InferenceHttpError {
    fn from_request(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout(error.to_string())
        } else {
            Self::Request(error.to_string())
        }
    }

    fn message(&self, secret: &str) -> String {
        let message = match self {
            Self::Timeout(message) => message,
            Self::Request(message) => message,
            Self::Response(message) => message,
        };
        if secret.is_empty() {
            return message.clone();
        }
        message.replace(secret, "[redacted]")
    }
}

/// How long to wait before retrying, or `None` when the response is not a reason to retry.
///
/// 429 and 503 are the two a provider uses for "later"; everything else is a request that will fail again the same way.
/// `Retry-After` is honoured when the provider sends it; a default stands in when it does not.
fn retry_after(status: u16, headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if status != 429 && status != 503 {
        return None;
    }
    let from_header = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs.min(60)));
    Some(from_header.unwrap_or(Duration::from_secs(5)))
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    response_format: ResponseFormat,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

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
}
