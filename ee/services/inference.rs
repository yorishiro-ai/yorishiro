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

    pub fn with_http(
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
pub trait InferenceHttp: Send + Sync {
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

pub struct InferenceHttpResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Pin<Box<dyn Future<Output = Result<Vec<u8>, InferenceHttpError>> + Send>>,
}

pub enum InferenceHttpError {
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
