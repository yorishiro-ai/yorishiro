//! Calling an LLM to propose values for fields an entity is missing.
//! Errors use `ProviderUnreachable`/`ProviderBusy` (`yorishiro_core::services::embedding::openai`), since a request that never reached the provider or one that answered "come back later" are both operator-actionable, not internal server errors.
//!
//! The one place this crate makes an outbound LLM call.
//! Everything else that reaches a model goes through `yorishiro_core::services::embedding`, which produces vectors rather than text.
//!
//! The credentials belong to a workspace, not to the deployment: this product does not pay for inference, so a workspace that wants inferred values brings its own key.
//! A workspace with no key configured gets a `ValidationFailed` rather than a silent fall back to `default` values: a caller who asked for inference and received defaults would have no way to tell that nothing was inferred.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use yorishiro_core::error::{ResultExt, YorishiroError};

/// Longer than the embedding provider's 30s: a chat completion over several fields is a slower call than embedding one string, and the work is already asynchronous behind a job id, so a caller is not sitting on this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// An OpenAI-compatible chat-completions endpoint, configured per workspace.
///
/// The same shape `yorishiro_core::services::embedding::OpenAiCompatibleConfig` takes, so a deployment pointing at Ollama or LM Studio configures both the same way.
#[derive(Clone)]
pub struct InferenceConfig {
    /// Example: `https://api.openai.com/v1` (a trailing `/` is optional).
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

/// Written out rather than derived, because a derived `Debug` prints `api_key` in clear text and anything that formats this (a tracing field, an error context, one `dbg!` left behind) would put a workspace's credential into a log.
/// The endpoint and model still show, since those are what a reader is usually trying to identify.
impl std::fmt::Debug for InferenceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InferenceConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

pub struct InferenceClient {
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

        Self {
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

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .map_err(|err| YorishiroError::ProviderUnreachable {
                url: self.base_url.clone(),
                message: err.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            if let Some(after) = retry_after(status.as_u16(), response.headers()) {
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

        let body: ChatResponse = response.json().await.internal()?;
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
    use super::*;

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
        let client = InferenceClient::new(InferenceConfig {
            base_url: "http://127.0.0.1:1/v1".into(),
            model: "unused".into(),
            api_key: "ysr-secret-value".into(),
        });

        let error = client
            .propose_fields(&serde_json::json!({"title": "x"}), &["category"])
            .await
            .expect_err("an unroutable provider must fail");

        let rendered = format!("{error:?}");
        assert!(
            !rendered.contains("ysr-secret-value"),
            "the api key must never appear in an error: {rendered}"
        );
    }

    /// `InferenceConfig` holds a workspace's API key, and a derived `Debug` would print it in clear text into any log formatting the struct.
    ///
    /// Asserted rather than left to the hand-written impl staying hand-written: adding `Debug` back to the derive list is a one-word edit that reads as tidying up.
    #[test]
    fn debug_does_not_render_the_api_key() {
        let config = InferenceConfig {
            base_url: "https://api.example.com/v1".into(),
            model: "gpt-4o-mini".into(),
            api_key: "sk-must-not-appear".into(),
        };

        let rendered = format!("{config:?}");

        assert!(
            !rendered.contains("sk-must-not-appear"),
            "the key reached Debug output: {rendered}"
        );
        // The endpoint and model still show: redaction should not cost the fields a reader wants.
        assert!(rendered.contains("api.example.com"));
        assert!(rendered.contains("gpt-4o-mini"));
    }
}
