//! Embedding provider abstraction, ported from master's `services::embedding`.
//!
//! Master also ships a local ONNX provider (`services::embedding::onnx`) as the zero-dependency
//! default. Not ported here: it needs model files (`models/model.onnx`, `models/tokenizer.json`)
//! that are not bundled and must be fetched separately, and this deployment's actually-reachable
//! embedding backend is LM Studio (OpenAI-compatible, confirmed reachable at `10.0.3.200:1234`
//! this session), which the `openai` provider already serves. Add ONNX back if a deployment needs
//! to run with no external embedding service at all.

pub mod openai;
pub mod sync;

pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

use async_trait::async_trait;

use crate::error::YorishiroError;

/// What a piece of text is being embedded for.
///
/// Asymmetric models (Qwen3-Embedding among them) expect a search query to carry an instruction
/// prefix that a stored document must not have. Embedding both the same way costs nothing
/// visible: the vectors are the right shape and normalize, the results are just worse. Providers
/// that need no such distinction ignore this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// Text a user or agent is searching with.
    Query,
    /// Text being stored and later searched for.
    Document,
}

/// Provider that generates embedding vectors.
/// The `content_entities.embedding` column is dimensionless (`vector`), so any model works. All
/// vectors in a deployment must share the same dimension count.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn dimensions(&self) -> usize;

    /// Must return vectors in the same order and count as the input.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError>;

    /// Embeds `text` knowing what it is for.
    ///
    /// The default ignores `kind` and delegates to [`Self::embed`], which is correct for every
    /// symmetric model. A provider whose model treats queries and documents differently
    /// overrides this.
    async fn embed_as(&self, kind: EmbedKind, text: &str) -> Result<Vec<f32>, YorishiroError> {
        let _ = kind;
        self.embed(text).await
    }

    /// Default implementation delegates to `embed_batch`.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, YorishiroError> {
        let batch = self.embed_batch(&[text]).await?;
        batch.into_iter().next().ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned no vectors for a single input"
            ))
        })
    }
}

/// A provider that satisfies the dimension count but errors on every actual call. Stands in
/// when `YORISHIRO_EMBEDDING_BASE_URL`/`YORISHIRO_EMBEDDING_MODEL` are unset, so a deployment
/// (or a test booting through the real `Hooks::after_context`) with no embedding backend
/// configured still boots; search/recall simply error if invoked, rather than boot itself
/// failing.
pub struct UnconfiguredEmbeddingProvider {
    dimensions: usize,
}

#[async_trait]
impl EmbeddingProvider for UnconfiguredEmbeddingProvider {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::ProviderUnreachable {
            url: String::new(),
            message: "no embedding provider is configured: set YORISHIRO_EMBEDDING_BASE_URL \
                      and YORISHIRO_EMBEDDING_MODEL"
                .into(),
        })
    }
}

/// Builds the embedding provider from environment variables.
///
/// `YORISHIRO_EMBEDDING_BASE_URL`/`YORISHIRO_EMBEDDING_MODEL` select the OpenAI-compatible
/// provider (LM Studio, Ollama, vLLM, or real OpenAI); when either is unset, boot proceeds with
/// [`UnconfiguredEmbeddingProvider`] rather than failing (see its doc comment).
/// `YORISHIRO_EMBEDDING_DIMENSIONS` defaults to 768, matching this deployment's migrated
/// `vector(768)` HNSW index (`migration/src/m20260822_100900_content_entities.rs`) and LM
/// Studio's `text-embedding-qwen3-embedding-0.6b`.
pub fn build_embedding_provider() -> anyhow::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    let dimensions: usize = std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "768".into())
        .parse()?;

    let base_url = std::env::var("YORISHIRO_EMBEDDING_BASE_URL").ok();
    let model = std::env::var("YORISHIRO_EMBEDDING_MODEL").ok();
    let (base_url, model) = match (base_url, model) {
        (Some(base_url), Some(model)) => (base_url, model),
        _ => {
            tracing::info!(
                "no embedding provider configured (YORISHIRO_EMBEDDING_BASE_URL/YORISHIRO_EMBEDDING_MODEL unset)"
            );
            return Ok(std::sync::Arc::new(UnconfiguredEmbeddingProvider {
                dimensions,
            }));
        }
    };

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: base_url.clone(),
        api_key: std::env::var("YORISHIRO_EMBEDDING_API_KEY").unwrap_or_default(),
        model: model.clone(),
        dimensions,
        send_dimensions_param: std::env::var("YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM")
            .map(|v| v == "true")
            .unwrap_or(false),
    });
    tracing::info!(provider = "openai", %base_url, %model, dimensions, "embedding provider configured");
    Ok(std::sync::Arc::new(provider))
}
