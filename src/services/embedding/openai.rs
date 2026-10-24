use std::time::Duration;

use loco_rs::prelude::async_trait;
use reqwest::StatusCode;
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
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dimensions: usize,
    send_dimensions_param: bool,
}

impl OpenAiCompatibleProvider {
    ///
    /// # Panics
    /// Panics if an internal invariant required by this operation is violated.
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("reqwest client configuration is static and always valid");

        Self::with_client(config, client)
    }

    pub fn with_client(config: OpenAiCompatibleConfig, client: reqwest::Client) -> Self {
        Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key,
            model: config.model,
            dimensions: config.dimensions,
            send_dimensions_param: config.send_dimensions_param,
        }
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
    index: usize,
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
                message: redact(&err.to_string(), &self.api_key),
            })?;
        let response = self.client.execute(request).await.map_err(|err| {
            YorishiroError::ProviderUnreachable {
                url: self.base_url.clone(),
                message: redact(&err.to_string(), &self.api_key),
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            // A provider saying "too many requests" or "temporarily unavailable" is saying to come back, different from a request it will never accept.
            // Told apart here so the caller can wait instead of dropping the work.
            if let Some(after) = retry_after(status.as_u16(), response.headers()) {
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
        let body = response.bytes().await.map_err(|err| {
            YorishiroError::Internal(anyhow::anyhow!(redact(&err.to_string(), &self.api_key)))
        })?;
        let mut parsed: EmbeddingsResponse = serde_json::from_slice(&body).internal()?;

        parsed.data.sort_unstable_by_key(|datum| datum.index);
        if parsed
            .data
            .iter()
            .enumerate()
            .any(|(index, datum)| datum.index != index)
        {
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned invalid vector indices"
            )));
        }

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

pub(crate) fn redact(message: &str, secret: &str) -> String {
    if secret.is_empty() {
        message.to_owned()
    } else {
        message.replace(secret, "[redacted]")
    }
}

/// How long to wait before retrying, or `None` when the response is not a reason to retry.
///
/// 429 and 503 are the two the providers use for "later"; everything else is a request that will fail again the same way.
/// `Retry-After` is honoured when the provider sends it; a default stands in when it does not.
pub(crate) fn retry_after(status: u16, headers: &reqwest::header::HeaderMap) -> Option<Duration> {
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
