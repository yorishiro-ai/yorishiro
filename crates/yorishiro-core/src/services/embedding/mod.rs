use async_trait::async_trait;

use crate::error::YorishiroError;

pub mod onnx;
pub mod openai;
pub mod sync;

pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

/// What a piece of text is being embedded for.
///
/// Asymmetric models — Qwen3-Embedding among them — expect a search query to carry an
/// instruction prefix that a stored document must not have. Embedding both the same way costs
/// nothing visible: the vectors are the right shape and normalize, the results are just worse.
/// Providers that need no such distinction ignore this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// Text a user or agent is searching with.
    Query,
    /// Text being stored and later searched for.
    Document,
}

/// Provider that generates embedding vectors. The `entities.embedding` column
/// is dimensionless (`vector`), so any model works. All vectors in a deployment
/// must share the same dimension count (`YSR_EMBEDDING_DIMENSIONS`).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn dimensions(&self) -> usize;

    /// Must return vectors in the same order and count as the input.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError>;

    /// Embeds `text` knowing what it is for.
    ///
    /// The default ignores `kind` and delegates to [`Self::embed`], which is correct for every
    /// symmetric model — including this crate's defaults. A provider whose model treats queries
    /// and documents differently overrides this; one that does not needs no changes, which is
    /// why the method carries a default rather than being required.
    ///
    /// Callers say what the text is for and nothing more. What gets prepended, if anything, is
    /// the model's own convention and belongs to the provider.
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

#[cfg(test)]
#[path = "../../../tests/services/embedding/mod.rs"]
mod tests;
