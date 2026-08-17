use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::EmbeddingProvider;
use crate::error::{ResultExt, YorishiroError};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct OpenAiCompatibleConfig {
    /// Example: `https://api.openai.com/v1` (a trailing `/` is optional).
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: usize,
    /// Some OpenAI-compatible implementations (vLLM, Ollama, etc.) don't recognize the `dimensions` parameter, so callers can explicitly choose whether to include it in the request.
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
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("reqwest client configuration is static and always valid");

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
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleProvider {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&EmbeddingsRequest {
                model: &self.model,
                input: texts,
                dimensions: self.send_dimensions_param.then_some(self.dimensions),
            })
            .send()
            .await
            // Not `.internal()`: a request that never reached the provider is a configuration or outage problem the operator can act on, and `internal server error` tells them nothing.
            // `send` fails before any HTTP status exists, so this arm is exactly the "could not be reached" case and never a provider that answered.
            .map_err(|err| YorishiroError::ProviderUnreachable {
                url: self.base_url.clone(),
                message: err.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            // A provider saying "too many requests" or "temporarily unavailable" is saying to come back, which is different from a request it will never accept.
            // Told apart here so the caller can wait instead of dropping the work: an embedding lost to a rate limit leaves the entity out of search until someone runs a resync, with nothing but a log line to say it happened.
            if let Some(after) = retry_after(status.as_u16(), response.headers()) {
                let body = response.text().await.unwrap_or_default();
                return Err(YorishiroError::ProviderBusy {
                    message: format!("embedding provider returned HTTP {status}: {body}"),
                    retry_after: after,
                });
            }
            let body = response.text().await.unwrap_or_default();
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned HTTP {status}: {body}"
            )));
        }

        let parsed: EmbeddingsResponse = response.json().await.internal()?;

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

#[cfg(test)]
#[path = "../../../tests/services/embedding/openai.rs"]
mod tests;

/// How long to wait before retrying, or `None` when the response is not a reason to retry.
///
/// 429 and 503 are the two the providers use for "later"; everything else is a request that will fail again the same way.
/// `Retry-After` is honoured when the provider sends it, since the provider knows its own window, and a default stands in when it does not: a missing header is not a reason to hammer.
fn retry_after(status: u16, headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if status != 429 && status != 503 {
        return None;
    }
    let from_header = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        // A provider asking for an hour is a provider this process should not sit waiting on; the work is recoverable by a resync, an unbounded wait is not.
        .map(|secs| Duration::from_secs(secs.min(60)));
    Some(from_header.unwrap_or(Duration::from_secs(5)))
}
