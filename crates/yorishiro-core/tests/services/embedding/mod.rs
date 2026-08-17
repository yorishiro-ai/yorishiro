use async_trait::async_trait;

use super::*;
use crate::error::YorishiroError;

struct StubProvider;

#[async_trait]
impl EmbeddingProvider for StubProvider {
    fn dimensions(&self) -> usize {
        3
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts
            .iter()
            .map(|t| vec![t.len() as f32, 0.0, 0.0])
            .collect())
    }
}

struct EmptyProvider;

#[async_trait]
impl EmbeddingProvider for EmptyProvider {
    fn dimensions(&self) -> usize {
        3
    }

    /// Violates the trait's "same order and count as the input" contract on purpose.
    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(vec![])
    }
}

/// `embed` is a provided method: every implementor gets it for free by delegating to `embed_batch`, so the delegation itself is what needs covering.
#[tokio::test]
async fn embed_delegates_to_embed_batch_and_returns_the_first_vector() {
    let embedded = StubProvider.embed("abcd").await.unwrap();

    assert_eq!(embedded, vec![4.0, 0.0, 0.0]);
}

/// A provider returning nothing would otherwise surface as a panic or a silently wrong vector; the default implementation turns it into an `Internal` error instead.
#[tokio::test]
async fn embed_reports_an_internal_error_when_the_provider_returns_nothing() {
    let error = EmptyProvider.embed("anything").await.unwrap_err();

    assert!(matches!(error, YorishiroError::Internal(_)));
}
