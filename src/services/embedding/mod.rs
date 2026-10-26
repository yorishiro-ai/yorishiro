//! Embedding provider trait and implementations.

pub mod local;
pub mod model_fetch;
pub mod openai;

pub(crate) const DEFAULT_EMBEDDING_DIMENSIONS: usize = 768;

/// Provider identity used for workspace stamp comparisons without constructing a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddingMetadata {
    pub model: String,
    pub dimensions: usize,
}

/// Resolves deployment provider identity without loading local model files.
///
/// # Errors
/// Returns an error when the configured provider metadata is incomplete or invalid.
pub fn deployment_embedding_metadata(
    settings: &crate::data::settings::Settings,
) -> anyhow::Result<Option<EmbeddingMetadata>> {
    let config = &settings.embedding;
    if let (Some(_), Some(model)) = (&config.base_url, &config.model) {
        return Ok(Some(EmbeddingMetadata {
            model: model.clone(),
            dimensions: config.dimensions,
        }));
    }
    match config.provider {
        crate::data::settings::EmbeddingProvider::None => Ok(None),
        crate::data::settings::EmbeddingProvider::Openai => {
            anyhow::bail!("embedding provider openai requires base_url and model")
        }
        crate::data::settings::EmbeddingProvider::Local => {
            let def = if config.local_model == model_fetch::DEFAULT_MODEL.short_id {
                model_fetch::DEFAULT_MODEL
            } else {
                anyhow::bail!("unknown local embedding model {:?}", config.local_model)
            };
            if config.dimensions != def.dimensions {
                anyhow::bail!(
                    "configured embedding dimensions do not match local model output width"
                )
            }
            Ok(Some(EmbeddingMetadata {
                model: def.id.to_owned(),
                dimensions: def.dimensions,
            }))
        }
    }
}

pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

use loco_rs::prelude::async_trait;

use crate::error::YorishiroError;

/// What a piece of text is being embedded for.
///
/// Asymmetric models expect a search query to carry an instruction prefix that a stored document must not have.
/// Embedding both the same way costs nothing visible: the vectors are the right shape and normalize, the results are just worse.
/// Providers that need no such distinction ignore this.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// Text a user or agent is searching with.
    Query,
    /// Text being stored and later searched for.
    Document,
}

/// Whether a provider is available for document and query embedding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingProviderAvailability {
    Enabled,
    Disabled,
}

/// Provider that generates embedding vectors.
/// All vectors written by a deployment must use one of the pre-created supported widths.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn availability(&self) -> EmbeddingProviderAvailability {
        EmbeddingProviderAvailability::Enabled
    }

    fn dimensions(&self) -> usize;

    /// Identifies the model this provider embeds with, for stamping onto a workspace at first successful embed.
    ///
    /// Each implementation names itself rather than a caller inferring it from configuration, so a caller that reads the deployed model regardless of which provider it actually got would record the wrong name.
    fn model_name(&self) -> String;

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

/// A provider that satisfies the dimension count but errors on every actual call.
/// Stands in when no embedding backend could be configured, so boot succeeds; search/recall simply error if invoked.
pub struct UnconfiguredEmbeddingProvider {
    dimensions: usize,
    /// What this particular deployment should set to get a working provider.
    ///
    /// Describes the provider configuration needed to enable embedding.
    /// The boot log says the same thing, but it scrolls away, and this error is what an operator keeps hitting afterwards.
    remedy: &'static str,
}

#[async_trait]
impl EmbeddingProvider for UnconfiguredEmbeddingProvider {
    fn availability(&self) -> EmbeddingProviderAvailability {
        EmbeddingProviderAvailability::Disabled
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> String {
        "unconfigured".into()
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::ProviderUnreachable {
            url: String::new(),
            message: format!("no embedding provider is configured: {}", self.remedy),
        })
    }
}

/// Builds the embedding provider from resolved settings.
///
/// Priority order:
///
/// 1. `YORISHIRO_EMBEDDING_BASE_URL` + `YORISHIRO_EMBEDDING_MODEL` both set — the OpenAI-compatible provider.
///    This is the documented shape for an external embedding backend (LM Studio, Ollama, vLLM, or real OpenAI).
/// 2. `YORISHIRO_EMBEDDING_PROVIDER=none` — explicitly disabled, returns [`UnconfiguredEmbeddingProvider`].
/// 3. `YORISHIRO_EMBEDDING_PROVIDER=local` — the local in-process provider (candle-based safetensors).
///
/// The configured default is the local provider; an explicit OpenAI provider requires both URL and model.
///
/// `YORISHIRO_EMBEDDING_DIMENSIONS` defaults to 768.
///
/// Production workers load the provider; API and MCP search requests enqueue query embedding work.
///
/// # Errors
/// Returns an error when the selected provider cannot be built, for example an unknown local model or model files that cannot be fetched.
pub async fn build_embedding_provider(
    config: &crate::data::settings::Settings,
) -> anyhow::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    let config = &config.embedding;
    let dimensions = config.dimensions;

    // Check base_url/model presence before PROVIDER so that the documented
    // shape (PROVIDER unset, base_url+model set) reaches the OpenAI-compatible
    // path rather than flowing into the local provider.
    let base_url = config.base_url.clone();
    let model = config.model.clone();

    if let (Some(base_url), Some(model)) = (base_url, model) {
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: base_url.clone(),
            api_key: config.api_key.clone(),
            model: model.clone(),
            dimensions,
            send_dimensions_param: config.send_dimensions_param,
        });
        tracing::info!(provider = "openai", %base_url, %model, dimensions, "embedding provider configured");
        return Ok(std::sync::Arc::new(provider));
    }

    match config.provider {
        crate::data::settings::EmbeddingProvider::None => {
            tracing::info!(
                "embedding provider explicitly disabled (YORISHIRO_EMBEDDING_PROVIDER=none)"
            );
            Ok(std::sync::Arc::new(UnconfiguredEmbeddingProvider {
                dimensions,
                remedy: "YORISHIRO_EMBEDDING_PROVIDER is set to \"none\"; unset it or set YORISHIRO_EMBEDDING_PROVIDER=local or YORISHIRO_EMBEDDING_BASE_URL/YORISHIRO_EMBEDDING_MODEL to enable embeddings",
            }))
        }
        crate::data::settings::EmbeddingProvider::Local => build_local_provider(config).await,
        crate::data::settings::EmbeddingProvider::Openai => {
            tracing::info!(
                "no explicit embedding configuration (YORISHIRO_EMBEDDING_BASE_URL/YORISHIRO_EMBEDDING_MODEL/YORISHIRO_EMBEDDING_PROVIDER unset); defaulting to local provider with recommended model"
            );
            anyhow::bail!("embedding provider openai requires base_url and model")
        }
    }
}

/// Picks a [`model_fetch::LocalModelDef`] by `YORISHIRO_LOCAL_MODEL`'s value, or [`model_fetch::DEFAULT_MODEL`] when unset.
/// An unrecognized value fails startup rather than silently falling back to the default: a typo in this variable is exactly the kind of "this deployment thinks it configured one model but got another" mistake the whole write-time model check exists to catch, and catching the typo at boot is strictly better than catching the resulting stamp mismatch on the first write.
fn resolve_local_model(requested: &str) -> anyhow::Result<&'static model_fetch::LocalModelDef> {
    if requested == model_fetch::DEFAULT_MODEL.short_id {
        Ok(model_fetch::DEFAULT_MODEL)
    } else {
        anyhow::bail!(
            "YORISHIRO_LOCAL_MODEL={requested:?} is not a known local model; valid values are: {}",
            model_fetch::DEFAULT_MODEL.short_id
        )
    }
}

/// `YORISHIRO_EMBEDDING_PROVIDER=local`'s branch of [`build_embedding_provider`].
/// `YORISHIRO_LOCAL_MODEL` selects which [`model_fetch::LocalModelDef`] to load (default: [`model_fetch::DEFAULT_MODEL`]); see [`resolve_local_model`].
/// The model and tokenizer files are fetched into `models/<short_id>/` on first use when absent.
///
/// There is no pooling variable: every model this provider can load is mean-pooled (see `local::LocalEmbeddingProvider`'s own doc comment for why that stays a fact rather than a config surface).
///
/// Missing model files are downloaded and verified before loading.
async fn build_local_provider(
    config: &crate::data::settings::Embedding,
) -> anyhow::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    let dimensions = config.dimensions;
    let def = resolve_local_model(&config.local_model)?;
    if dimensions != def.dimensions {
        anyhow::bail!(
            "YORISHIRO_EMBEDDING_DIMENSIONS={dimensions} does not match {}'s own output width \
             ({}); unset YORISHIRO_EMBEDDING_DIMENSIONS to use the selected model's own width, \
             or point YORISHIRO_LOCAL_MODEL at a model that actually produces {dimensions}",
            def.id,
            def.dimensions
        );
    }
    let max_sequence_length = config.local_max_sequence_length;
    let (model_path, tokenizer_path) = model_fetch::ensure_model_files(def).await?;
    let model_path = model_path.display().to_string();
    let tokenizer_path = tokenizer_path.display().to_string();

    let local_config = local::LocalEmbeddingConfig {
        model_path: model_path.clone().into(),
        tokenizer_path: tokenizer_path.clone().into(),
        def,
        max_sequence_length,
    };
    let provider =
        tokio::task::spawn_blocking(move || local::LocalEmbeddingProvider::load(local_config))
            .await
            .map_err(|err| anyhow::anyhow!("local embedding model loader task failed: {err}"))?
            .map_err(|err| {
                anyhow::anyhow!(
                    "{err}\n\nthe local embedding provider needs '{model_path}' and \
             '{tokenizer_path}', or set YORISHIRO_EMBEDDING_PROVIDER=openai to use an \
             OpenAI-compatible endpoint instead"
                )
            })?;
    tracing::info!(provider = "local", %model_path, dimensions, "embedding provider configured");
    Ok(std::sync::Arc::new(provider))
}
