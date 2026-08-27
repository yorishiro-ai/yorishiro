//! Embedding provider abstraction.

pub mod onnx;
pub mod openai;
pub mod sync;

pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::YorishiroError;

/// What a piece of text is being embedded for.
///
/// Asymmetric models expect a search query to carry an instruction prefix that a stored document must not have.
/// Embedding both the same way costs nothing visible: the vectors are the right shape and normalize, the results are just worse.
/// Providers that need no such distinction ignore this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// Text a user or agent is searching with.
    Query,
    /// Text being stored and later searched for.
    Document,
}

/// Provider that generates embedding vectors.
/// The `content_entities.embedding` column is dimensionless (`vector`), so any model works.
/// All vectors in a deployment must share the same dimension count.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn dimensions(&self) -> usize;

    /// How many tokens `text` costs this provider, for quota purposes.
    ///
    /// The default is a byte-length estimate, and deliberately so: a provider without a tokenizer in the process (an external API, where the model runs elsewhere) cannot count exactly, and loading one purely to meter would mean shipping a tokenizer to a deployment that chose not to run embeddings locally.
    ///
    /// Four bytes per token is the usual English rule of thumb and overestimates Japanese text, which suits a quota: overcharging throttles a heavy caller early, while undercharging lets it past the limit it was supposed to hit.
    fn count_tokens(&self, text: &str) -> u32 {
        u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
    }

    /// Must return vectors in the same order and count as the input.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError>;

    /// Embeds `text` knowing what it is for.
    ///
    /// The default ignores `kind` and delegates to [`Self::embed`], which is correct for every symmetric model.
    /// A provider whose model treats queries and documents differently overrides this.
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

/// Resolves a workspace's own embedding provider, if it has one.
///
/// A seam: a deployment can let a workspace point at a different embedding backend than the deployment default (its own ONNX node, a different OpenAI-compatible endpoint) without touching the callers that resolve a provider.
/// [`DefaultEmbeddingResolver`] is the behaviour of every deployment that does not replace it: every workspace uses the deployment-wide provider.
///
/// `conn` is `ctx.db` (Loco's own `DatabaseConnection`), not the RLS-scoped tenant pool: a per-workspace assignment is deployment configuration, read the same way `identity_workspace_llm_keys` is, not tenant content.
/// This is why `conn` takes a `sea_orm::DatabaseConnection` rather than `DbHandle`: `DbHandle` does not exist on SQLite (see `Hooks::after_context`), and this seam must work on both backends, unlike `Authenticator`, which is a PostgreSQL/RLS-only concept by design.
///
/// Returns `Ok(None)` when the workspace has no assignment of its own, so the caller falls back to the deployment default already held in `shared_store` rather than this seam constructing it: building the fallback (an ONNX model load can be hundreds of megabytes) is a cost only worth paying once, not on every call whether or not a workspace override exists.
/// No caching: this runs once per call, same as `identity_workspace_llm_keys::get`.
/// Acceptable for the same reason it is there: a metadata read, not the slow work.
#[async_trait]
pub trait WorkspaceEmbeddingResolver: Send + Sync {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<Arc<dyn EmbeddingProvider>>, YorishiroError>;
}

/// This crate's own rule: no workspace has its own provider, so every caller falls back to the deployment default.
pub struct DefaultEmbeddingResolver;

#[async_trait]
impl WorkspaceEmbeddingResolver for DefaultEmbeddingResolver {
    async fn resolve(
        &self,
        _conn: &sea_orm::DatabaseConnection,
        _workspace_id: Uuid,
    ) -> Result<Option<Arc<dyn EmbeddingProvider>>, YorishiroError> {
        Ok(None)
    }
}

/// The resolver a deployment gets when it does not choose one.
pub fn default_embedding_resolver() -> Arc<dyn WorkspaceEmbeddingResolver> {
    Arc::new(DefaultEmbeddingResolver)
}

/// A provider that satisfies the dimension count but errors on every actual call.
/// Stands in when `YORISHIRO_EMBEDDING_BASE_URL`/`YORISHIRO_EMBEDDING_MODEL` are unset, so boot succeeds with no embedding backend configured; search/recall simply error if invoked.
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

/// The model name this deployment is configured for, for stamping onto new workspaces.
/// Read from the environment rather than the provider, to avoid adding a `model()` method to the trait for this one caller.
pub fn model_name_from_env() -> String {
    std::env::var("YORISHIRO_EMBEDDING_MODEL").unwrap_or_else(|_| "unconfigured".into())
}

/// Builds the embedding provider from environment variables.
///
/// `YORISHIRO_EMBEDDING_PROVIDER=local` selects the local ONNX provider (needs `YORISHIRO_ONNX_MODEL_PATH`/`YORISHIRO_ONNX_TOKENIZER_PATH`, no external service).
/// Otherwise, `YORISHIRO_EMBEDDING_BASE_URL`/`YORISHIRO_EMBEDDING_MODEL` select the OpenAI-compatible provider (LM Studio, Ollama, vLLM, or real OpenAI); when either is unset, boot proceeds with [`UnconfiguredEmbeddingProvider`] rather than failing.
/// `YORISHIRO_EMBEDDING_DIMENSIONS` defaults to 768.
pub fn build_embedding_provider() -> anyhow::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    let dimensions: usize = std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "768".into())
        .parse()?;

    if std::env::var("YORISHIRO_EMBEDDING_PROVIDER").as_deref() == Ok("local") {
        return build_local_onnx_provider(dimensions);
    }

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

/// `YORISHIRO_EMBEDDING_PROVIDER=local`'s branch of [`build_embedding_provider`].
/// `YORISHIRO_ONNX_MODEL_PATH`/`YORISHIRO_ONNX_TOKENIZER_PATH` default to `models/model.onnx`/`models/tokenizer.json`; `YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH` defaults to 512.
/// `YORISHIRO_ONNX_POOLING` is rejected rather than defaulted on an unknown value: reading a model with the wrong pooling does not fail, it just returns worse vectors.
/// `YORISHIRO_ONNX_QUERY_INSTRUCTION` empty is treated as unset: an operator clearing the variable means "no prefix", not "prefix with nothing".
fn build_local_onnx_provider(
    dimensions: usize,
) -> anyhow::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    let max_sequence_length: usize = std::env::var("YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH")
        .unwrap_or_else(|_| "512".into())
        .parse()?;
    let model_path =
        std::env::var("YORISHIRO_ONNX_MODEL_PATH").unwrap_or_else(|_| "models/model.onnx".into());
    let tokenizer_path = std::env::var("YORISHIRO_ONNX_TOKENIZER_PATH")
        .unwrap_or_else(|_| "models/tokenizer.json".into());
    let pooling = match std::env::var("YORISHIRO_ONNX_POOLING") {
        Ok(value) => onnx::Pooling::parse(&value)?,
        Err(_) => onnx::Pooling::default(),
    };
    let query_instruction = std::env::var("YORISHIRO_ONNX_QUERY_INSTRUCTION")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let provider = onnx::LocalOnnxProvider::load(onnx::LocalOnnxConfig {
        model_path: model_path.clone().into(),
        tokenizer_path: tokenizer_path.clone().into(),
        dimensions,
        max_sequence_length,
        pooling,
        query_instruction,
    })
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}\n\nthe local ONNX embedding provider needs '{model_path}' and \
             '{tokenizer_path}': these are not bundled in the repository, and must be fetched \
             separately, or set YORISHIRO_EMBEDDING_PROVIDER=openai to use an OpenAI-compatible \
             endpoint instead"
        )
    })?;
    tracing::info!(provider = "local", %model_path, dimensions, ?pooling, "embedding provider configured");
    Ok(std::sync::Arc::new(provider))
}
