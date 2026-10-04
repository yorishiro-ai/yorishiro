use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Request, StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};

use super::EmbeddingProvider;
use crate::error::{ResultExt, YorishiroError};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(5);
const MAX_RETRY_AFTER_SECONDS: u64 = 60;

pub struct OpenAiCompatibleConfig {
    /// Example: `http://localhost:11434`
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: usize,
    /// Some OpenAI-compatible implementations (vLLM, Ollama, LM Studio) don't recognize the `dimensions` parameter, so callers can explicitly choose whether to include it.
    pub send_dimensions_param: bool,
}

pub struct OpenAiCompatibleProvider {
    http: Arc<dyn OpenAiCompatibleHttp>,
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dimensions: usize,
    send_dimensions_param: bool,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("reqwest client configuration is static and always valid");

        Self::with_http(config, client, Arc::new(ReqwestOpenAiHttp))
    }

    fn with_http(
        config: OpenAiCompatibleConfig,
        client: reqwest::Client,
        http: Arc<dyn OpenAiCompatibleHttp>,
    ) -> Self {
        Self {
            http,
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key,
            model: config.model,
            dimensions: config.dimensions,
            send_dimensions_param: config.send_dimensions_param,
        }
    }
}

#[async_trait]
trait OpenAiCompatibleHttp: Send + Sync {
    async fn send(
        &self,
        client: &reqwest::Client,
        request: Request,
    ) -> Result<OpenAiHttpResponse, OpenAiHttpError>;
}

struct ReqwestOpenAiHttp;

#[async_trait]
impl OpenAiCompatibleHttp for ReqwestOpenAiHttp {
    async fn send(
        &self,
        client: &reqwest::Client,
        request: Request,
    ) -> Result<OpenAiHttpResponse, OpenAiHttpError> {
        let response = client
            .execute(request)
            .await
            .map_err(OpenAiHttpError::from_request)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes();
        Ok(OpenAiHttpResponse {
            status,
            headers,
            body: Box::pin(async move {
                body.await
                    .map(|body| body.to_vec())
                    .map_err(|error| OpenAiHttpError::Response(error.to_string()))
            }),
        })
    }
}

struct OpenAiHttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Pin<Box<dyn Future<Output = Result<Vec<u8>, OpenAiHttpError>> + Send>>,
}

enum OpenAiHttpError {
    Timeout(String),
    Request(String),
    Response(String),
}

impl OpenAiHttpError {
    fn from_request(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout(error.to_string())
        } else {
            Self::Request(error.to_string())
        }
    }
}

impl OpenAiHttpError {
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

#[derive(Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleProvider {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let request = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&EmbeddingsRequest {
                model: &self.model,
                input: texts,
                dimensions: self.send_dimensions_param.then_some(self.dimensions),
            })
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
                OpenAiHttpError::Timeout(_) | OpenAiHttpError::Request(_) => {
                    YorishiroError::ProviderUnreachable {
                        url: self.base_url.clone(),
                        message: err.message(&self.api_key),
                    }
                }
                OpenAiHttpError::Response(_) => {
                    YorishiroError::Internal(anyhow::anyhow!(err.message(&self.api_key)))
                }
            })?;

        let status = response.status;
        if !status.is_success() {
            // A provider saying "too many requests" or "temporarily unavailable" is saying to come back, different from a request it will never accept.
            // Told apart here so the caller can wait instead of dropping the work.
            if let Some(after) = retry_after(status.as_u16(), &response.headers) {
                return Err(YorishiroError::ProviderBusy {
                    message: format!("embedding provider returned HTTP {status}"),
                    retry_after: after,
                });
            }
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned HTTP {status}"
            )));
        }

        // Keep body consumption after status handling so provider error responses remain unread.
        let body = response
            .body
            .await
            .map_err(|err| YorishiroError::Internal(anyhow::anyhow!(err.message(&self.api_key))))?;
        let parsed: EmbeddingsResponse = serde_json::from_slice(&body).internal()?;

        let vectors: Vec<Vec<f32>> = parsed.data.into_iter().map(|d| d.embedding).collect();

        if vectors.len() != texts.len() {
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned {} vectors for {} inputs",
                vectors.len(),
                texts.len()
            )));
        }

        for vector in &vectors {
            if vector.len() != self.dimensions {
                return Err(YorishiroError::Internal(anyhow::anyhow!(
                    "embedding provider returned a vector of length {} but expected {}",
                    vector.len(),
                    self.dimensions
                )));
            }
        }

        Ok(vectors)
    }
}

/// How long to wait before retrying, or `None` when the response is not a reason to retry.
///
/// 429 and 503 are the two the providers use for "later"; everything else is a request that will fail again the same way.
/// `Retry-After` is honoured when the provider sends it; a default stands in when it does not.
fn retry_after(status: u16, headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if status != StatusCode::TOO_MANY_REQUESTS.as_u16()
        && status != StatusCode::SERVICE_UNAVAILABLE.as_u16()
    {
        return None;
    }
    let from_header = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECONDS)));
    Some(from_header.unwrap_or(DEFAULT_RETRY_AFTER))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct FakeHttp {
        outcome: Mutex<Option<Result<OpenAiHttpResponse, OpenAiHttpError>>>,
        request: Mutex<Option<Request>>,
    }

    #[async_trait]
    impl OpenAiCompatibleHttp for FakeHttp {
        async fn send(
            &self,
            _client: &reqwest::Client,
            request: Request,
        ) -> Result<OpenAiHttpResponse, OpenAiHttpError> {
            *self.request.lock().unwrap() = Some(request);
            self.outcome.lock().unwrap().take().unwrap()
        }
    }

    fn config() -> OpenAiCompatibleConfig {
        OpenAiCompatibleConfig {
            base_url: "http://embedding.test/".into(),
            api_key: "test-api-key".into(),
            model: "test-model".into(),
            dimensions: 2,
            send_dimensions_param: true,
        }
    }

    fn response(status: u16, body: &str) -> OpenAiHttpResponse {
        OpenAiHttpResponse {
            status: StatusCode::from_u16(status).unwrap(),
            headers: HeaderMap::new(),
            body: Box::pin(std::future::ready(Ok(body.as_bytes().to_vec()))),
        }
    }

    fn body_error(message: &str) -> OpenAiHttpResponse {
        OpenAiHttpResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Box::pin(std::future::ready(Err(OpenAiHttpError::Response(
                message.into(),
            )))),
        }
    }

    fn provider(
        outcome: Result<OpenAiHttpResponse, OpenAiHttpError>,
    ) -> (OpenAiCompatibleProvider, Arc<FakeHttp>) {
        let http = Arc::new(FakeHttp {
            outcome: Mutex::new(Some(outcome)),
            request: Mutex::new(None),
        });
        let provider =
            OpenAiCompatibleProvider::with_http(config(), reqwest::Client::new(), http.clone());
        (provider, http)
    }

    #[tokio::test]
    async fn injected_transport_preserves_batch_request_and_order() {
        let (provider, http) = provider(Ok(response(
            200,
            r#"{"data":[{"embedding":[1.0,2.0]},{"embedding":[3.0,4.0]}]}"#,
        )));

        let vectors = provider.embed_batch(&["first", "second"]).await.unwrap();

        assert_eq!(vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        let request = http.request.lock().unwrap().take().unwrap();
        assert_eq!(request.url().as_str(), "http://embedding.test/embeddings");
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer test-api-key"
        );
        assert_eq!(
            request.body().unwrap().as_bytes().unwrap(),
            br#"{"model":"test-model","input":["first","second"],"dimensions":2}"#
        );
    }

    #[tokio::test]
    async fn malformed_response_is_internal_and_redacts_body() {
        let secret = "provider-secret-body";
        let (provider, _) = provider(Ok(response(200, secret)));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(&error, YorishiroError::Internal(_)));
        assert!(!format!("{error:?}").contains(secret));
        assert!(!format!("{error:?}").contains("test-api-key"));
    }

    #[tokio::test]
    async fn response_validation_rejects_wrong_count_and_dimensions() {
        let (first_provider, _) =
            provider(Ok(response(200, r#"{"data":[{"embedding":[1.0,2.0]}]}"#)));
        let error = first_provider
            .embed_batch(&["first", "second"])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("1 vectors for 2 inputs"));

        let (second_provider, _) = provider(Ok(response(200, r#"{"data":[{"embedding":[1.0]}]}"#)));
        let error = second_provider.embed_batch(&["text"]).await.unwrap_err();
        assert!(error.to_string().contains("length 1 but expected 2"));
    }

    #[tokio::test]
    async fn transport_failure_maps_to_provider_unreachable() {
        let (provider, _) = provider(Err(OpenAiHttpError::Request("connection refused".into())));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
        assert_eq!(error.code().unwrap().as_str(), "provider_unreachable");
        assert!(!format!("{error:?}").contains("test-api-key"));
    }

    #[tokio::test]
    async fn timeout_maps_to_provider_unreachable() {
        let timeout_message =
            "error sending request for url (https://example.invalid): operation timed out";
        let (provider, _) = provider(Err(OpenAiHttpError::Timeout(timeout_message.into())));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
        assert!(error.to_string().contains(timeout_message));
        assert!(!error.to_string().contains("test-api-key"));
    }

    #[tokio::test]
    async fn response_body_read_failure_maps_to_internal_and_redacts_api_key() {
        let (provider, _) = provider(Ok(body_error(
            "provider body read failed after echoing test-api-key",
        )));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(error, YorishiroError::Internal(_)));
        assert!(!error.to_string().contains("test-api-key"));
    }

    #[tokio::test]
    async fn provider_status_mapping_omits_body() {
        let (provider, _) = provider(Ok(response(503, "provider-secret-body")));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(
            &error,
            YorishiroError::ProviderBusy { retry_after, .. }
                if *retry_after == Duration::from_secs(5)
        ));
        assert!(!format!("{error:?}").contains("provider-secret-body"));
    }
}
