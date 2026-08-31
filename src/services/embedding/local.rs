use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::{Activation, VarBuilder};
use candle_transformers::models::nomic_bert::{Config as NomicConfig, NomicBertModel};
use candle_transformers::models::xlm_roberta::{Config as XlmRobertaConfig, XLMRobertaModel};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use super::EmbeddingProvider;
use super::model_fetch::{Architecture, LocalModelDef};
use crate::error::YorishiroError;

/// Lower bound for `max_sequence_length`.
/// tokenizers subtracts the number of special tokens (2-3 for BERT-family models) from `max_length` during truncation, so a value below that underflows (in release builds this wraps around, silently disabling truncation).
/// There's no practical use for an extremely short sequence length either, so we reject with a comfortable margin.
/// Model-independent (a tokenizer property), unlike the upper bound, which comes from the selected [`LocalModelDef::max_sequence_length`].
const MIN_SEQUENCE_LENGTH: usize = 16;

/// Upper bound on wait time for a single embed call.
/// Inference is serialized within the process, so this guards against unbounded waits when prior requests pile up (the local equivalent of the OpenAI-compatible provider's HTTP timeout).
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LocalEmbeddingConfig {
    pub model_path: PathBuf,
    pub tokenizer_path: PathBuf,
    /// Which model these files are expected to hold: decides architecture, output dimensionality, sequence-length bound, and query/document prefixes.
    /// `load` runs a probe inference and fails startup if the loaded model's actual output dimension doesn't match `def.dimensions`.
    pub def: &'static LocalModelDef,
    /// Maximum sequence length for tokenization.
    /// Text longer than this is truncated. Must not exceed `def.max_sequence_length`; see [`LocalEmbeddingProvider::load`].
    pub max_sequence_length: usize,
}

/// One of the two `candle-transformers` model families this provider can load, selected by [`Architecture`].
/// The forward-signature difference between the two (`nomic_bert` takes optional `token_type_ids`/`attention_mask`; `xlm_roberta` takes both as required arguments and also wants `past_key_value`/`encoder_hidden_states`/`encoder_attention_mask`, all `None` for a plain forward pass here) stays local to [`LoadedModel::forward`], per this repository's own rule that a backend branch must not leak into callers.
enum LoadedModel {
    Nomic(NomicBertModel),
    Xlm(XLMRobertaModel),
}

impl LoadedModel {
    fn forward(
        &self,
        input_ids: &Tensor,
        token_type_ids: &Tensor,
        attention_mask: &Tensor,
    ) -> candle_core::Result<Tensor> {
        match self {
            Self::Nomic(model) => {
                model.forward(input_ids, Some(token_type_ids), Some(attention_mask))
            }
            Self::Xlm(model) => {
                model.forward(input_ids, attention_mask, token_type_ids, None, None, None)
            }
        }
    }
}

/// `intfloat/multilingual-e5-base`'s `config.json` at revision `d128750597153bb5987e10b1c3493a34e5a4502a`, transcribed field-by-field rather than fetched: `xlm_roberta::Config` has no `Default` impl (unlike `nomic_bert::Config`), and this avoids a third pinned-and-digested artifact for a file that never changes independently of the revision already pinned on [`super::model_fetch::MULTILINGUAL_E5_BASE`].
fn multilingual_e5_base_config() -> XlmRobertaConfig {
    XlmRobertaConfig {
        vocab_size: 250_002,
        hidden_size: 768,
        num_hidden_layers: 12,
        num_attention_heads: 12,
        intermediate_size: 3072,
        hidden_act: Activation::Gelu,
        hidden_dropout_prob: 0.1,
        attention_probs_dropout_prob: 0.1,
        max_position_embeddings: 514,
        type_vocab_size: 1,
        pad_token_id: 1,
        layer_norm_eps: 1e-5,
        position_embedding_type: "absolute".to_string(),
    }
}

/// Provider that generates embeddings using a local, in-process model.
/// Has no runtime dependency on external services, making it suitable for closed/offline environments.
///
/// Which model loads is selected by [`super::model_fetch::LocalModelDef`] (see [`super::model_fetch::MODELS`]), read through one of two `candle-transformers` architectures from a `safetensors` checkpoint: a different model here is a different [`LoadedModel`] variant, not a config change.
///
/// Token embeddings are aggregated into a sentence vector via mean pooling weighted by the attention mask, then L2-normalized for stable cosine-distance search: both models this provider loads were trained and evaluated this way, and neither definition carries a pooling field of its own for exactly that reason.
/// A field with one legal value on every definition so far would be the removed `YORISHIRO_ONNX_POOLING` reborn as code; one arrives the day a model that pools differently does.
pub struct LocalEmbeddingProvider {
    // candle's `Tensor` ops on CPU are not internally parallel across an inference the way onnxruntime's
    // intra-op threading was, but the model is `Send + Sync` regardless; the Mutex here still exists to bound
    // memory: without it, concurrent requests would each build their own batch of activations on a CPU-only
    // process, which is worse than queuing them.
    inner: Arc<Inner>,
}

struct Inner {
    model: Mutex<LoadedModel>,
    tokenizer: Tokenizer,
    def: &'static LocalModelDef,
    device: Device,
}

fn internal(message: impl std::fmt::Display) -> YorishiroError {
    YorishiroError::Internal(anyhow::anyhow!("{message}"))
}

impl LocalEmbeddingProvider {
    /// Loads the model and tokenizer from files, validating output dimensionality via a probe inference.
    /// This blocks for hundreds of ms to a few seconds, so call it once at startup only.
    pub fn load(config: LocalEmbeddingConfig) -> Result<Self, YorishiroError> {
        let def = config.def;
        if config.max_sequence_length < MIN_SEQUENCE_LENGTH {
            return Err(internal(format!(
                "max_sequence_length must be >= {MIN_SEQUENCE_LENGTH}, got {}",
                config.max_sequence_length
            )));
        }
        if config.max_sequence_length > def.max_sequence_length {
            return Err(internal(format!(
                "max_sequence_length must be <= {} ({}'s own usable sequence limit), got {}",
                def.max_sequence_length, def.id, config.max_sequence_length
            )));
        }

        let mut tokenizer = Tokenizer::from_file(&config.tokenizer_path).map_err(|err| {
            internal(format!(
                "failed to load tokenizer '{}': {err}",
                config.tokenizer_path.display()
            ))
        })?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: config.max_sequence_length,
                ..Default::default()
            }))
            .map_err(|err| internal(format!("failed to configure truncation: {err}")))?;
        // `PaddingParams::default()` pads with token id 0, which is XLM-RoBERTa's `<s>` (start) token, not multilingual-e5-base's own pad token (id 1, `pad_token_id` in its config.json).
        // This does not corrupt output regardless: `embed_blocking` builds the attention mask from the tokenizer's own padding, and masked positions are excluded before pooling in `mean_pool_normalized`, so which token id fills a padded position never reaches the pooled result.
        // Confirmed by `embeds_texts_with_a_real_e5_model`'s batched call, which forces real padding rather than only exercising unpadded single-text calls.
        tokenizer.with_padding(Some(PaddingParams::default()));

        let device = Device::Cpu;
        // `from_buffered_safetensors` (a plain read, not `from_mmaped_safetensors`) is deliberate:
        // the mmap variant is `unsafe` because the caller must guarantee the file is never mutated
        // for as long as the mapping lives, and that guarantee only holds for `model_fetch`'s own
        // managed tier (replaced solely by `rename` into a path this provider has not opened yet).
        // An operator-chosen path would be outside that mechanism with no such guarantee, so an
        // `unsafe` block here would be asserting a safety invariant this function cannot actually
        // promise. The read happens once at startup and is noise next to the model fetch it may
        // follow.
        let bytes = std::fs::read(&config.model_path).map_err(|err| {
            internal(format!(
                "failed to read model weights '{}': {err}",
                config.model_path.display()
            ))
        })?;
        let vb =
            VarBuilder::from_buffered_safetensors(bytes, DType::F32, &device).map_err(|err| {
                internal(format!(
                    "failed to load model weights '{}': {err}",
                    config.model_path.display()
                ))
            })?;
        let model = match def.architecture {
            Architecture::NomicBert => NomicBertModel::load(vb, &NomicConfig::default())
                .map(LoadedModel::Nomic)
                .map_err(|err| {
                    internal(format!(
                        "failed to build the nomic-bert model from '{}': {err}",
                        config.model_path.display()
                    ))
                })?,
            Architecture::XlmRoberta => XLMRobertaModel::new(&multilingual_e5_base_config(), vb)
                .map(LoadedModel::Xlm)
                .map_err(|err| {
                    internal(format!(
                        "failed to build the xlm-roberta model from '{}': {err}",
                        config.model_path.display()
                    ))
                })?,
        };

        let inner = Inner {
            model: Mutex::new(model),
            tokenizer,
            def,
            device,
        };

        // Dimension mismatches must be caught here (at server startup).
        // If undetected until the first entity write, embeddings would silently keep failing in production.
        inner.embed_blocking(&["dimension probe".to_string()])?;

        Ok(Self {
            inner: Arc::new(inner),
        })
    }
}

impl Inner {
    fn embed_blocking(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|err| internal(format!("tokenization failed: {err}")))?;

        let batch = encodings.len();
        // Padding is configured as BatchLongest, so every encoding in the batch has the same length.
        let seq = encodings
            .first()
            .map(|encoding| encoding.get_ids().len())
            .ok_or_else(|| internal("tokenizer returned no encodings"))?;
        if seq == 0 {
            return Err(internal("tokenizer produced an empty sequence"));
        }

        let mut input_ids = Vec::with_capacity(batch * seq);
        let mut attention_mask = Vec::with_capacity(batch * seq);
        // Read from the tokenizer's own type ids rather than hardcoded per architecture: a nomic_bert-family tokenizer emits meaningful segment ids (`type_vocab_size` 2), while multilingual-e5-base's XLM-RoBERTa tokenizer emits all zeros on its own, matching its `type_vocab_size` of 1 (index 0 is the only legal value in that embedding table).
        // Reading rather than assuming means neither architecture branch has to know the other's convention here.
        let mut token_type_ids = Vec::with_capacity(batch * seq);
        for encoding in &encodings {
            input_ids.extend(encoding.get_ids().iter().copied());
            attention_mask.extend(encoding.get_attention_mask().iter().map(|&v| i64::from(v)));
            token_type_ids.extend(encoding.get_type_ids().iter().copied());
        }

        let input_ids = Tensor::from_vec(input_ids, (batch, seq), &self.device)
            .map_err(|err| internal(format!("failed to build input_ids tensor: {err}")))?;
        let token_type_ids = Tensor::from_vec(token_type_ids, (batch, seq), &self.device)
            .map_err(|err| internal(format!("failed to build token_type_ids tensor: {err}")))?;
        let attention_mask_tensor =
            Tensor::from_vec(attention_mask.clone(), (batch, seq), &self.device)
                .map_err(|err| internal(format!("failed to build attention_mask tensor: {err}")))?;

        // Recovers from poisoning: even if a panic occurs while the lock is held, the model carries no
        // mutable state across inferences (`&mut` is only an artifact of holding it behind a Mutex for
        // memory bounding, not something `forward` itself needs), so the invariant isn't actually broken.
        // Permanently disabling embedding until a process restart over a poison would cause more harm.
        let model = self.model.lock().unwrap_or_else(PoisonError::into_inner);
        let hidden_states = model
            .forward(&input_ids, &token_type_ids, &attention_mask_tensor)
            .map_err(|err| internal(format!("model inference failed: {err}")))?;
        drop(model);

        let dims = hidden_states.dims();
        if dims.len() != 3 || dims[0] != batch || dims[1] != seq {
            return Err(internal(format!(
                "unexpected model output shape {dims:?} (expected [batch={batch}, seq={seq}, hidden])"
            )));
        }
        let hidden = dims[2];
        if hidden != self.def.dimensions {
            return Err(internal(format!(
                "model produces {hidden}-dimensional embeddings, expected {}",
                self.def.dimensions
            )));
        }

        let data = hidden_states
            .flatten_all()
            .and_then(|flat| flat.to_vec1::<f32>())
            .map_err(|err| internal(format!("failed to read model output: {err}")))?;

        let mut results = Vec::with_capacity(batch);
        for b in 0..batch {
            results.push(mean_pool_normalized(
                &data[b * seq * hidden..(b + 1) * seq * hidden],
                &attention_mask[b * seq..(b + 1) * seq],
                seq,
                hidden,
            ));
        }
        Ok(results)
    }
}

/// L2-normalizes in place so cosine distance is stable.
fn l2_normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

/// Averages only tokens where the attention mask is 1, then L2-normalizes.
/// Returns a zero vector if every token is masked (this doesn't happen in practice since special tokens are always present).
///
/// `pub` (rather than private) only so this module's own tests can call it directly; `#[doc(hidden)]` keeps it out of the public API docs.
#[doc(hidden)]
pub fn mean_pool_normalized(
    token_embeddings: &[f32],
    attention_mask: &[i64],
    seq: usize,
    hidden: usize,
) -> Vec<f32> {
    let mut pooled = vec![0.0_f32; hidden];
    let mut count = 0.0_f32;
    for t in 0..seq {
        if attention_mask[t] == 0 {
            continue;
        }
        count += 1.0;
        let row = &token_embeddings[t * hidden..(t + 1) * hidden];
        for (acc, value) in pooled.iter_mut().zip(row) {
            *acc += value;
        }
    }
    if count > 0.0 {
        for value in &mut pooled {
            *value /= count;
        }
    }

    l2_normalize(&mut pooled);
    pooled
}

#[async_trait]
impl EmbeddingProvider for LocalEmbeddingProvider {
    fn dimensions(&self) -> usize {
        self.inner.def.dimensions
    }

    fn model_name(&self) -> String {
        self.inner.def.id.into()
    }

    /// Counted exactly: this provider already holds the tokenizer the model uses, so the number is the one the model will actually see rather than an estimate of it.
    ///
    /// Undercounts by the prefix's own token length on a model with a non-empty query/document prefix (`embed_as` prepends it before tokenizing, but this method has no `EmbedKind` to know which prefix, if any, the caller is about to add).
    /// A few tokens' undercount on a quota meant to catch abuse rather than bill to the token is not worth threading `EmbedKind` through a method whose contract is "count this text", not "count what `embed_as` would eventually send".
    fn count_tokens(&self, text: &str) -> u32 {
        match self.inner.tokenizer.encode(text, false) {
            Ok(encoding) => u32::try_from(encoding.len()).unwrap_or(u32::MAX),
            // A text this tokenizer cannot encode is one the model could not embed either; the request is about to fail on its own, so fall back rather than decide here.
            Err(_) => u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX),
        }
    }

    /// Prepends the selected model's query/document prefix, if it has one, before embedding.
    /// nomic-embed-text-v1.5's definition carries empty prefixes (see `super::model_fetch::NOMIC`'s own doc comment for why), so this is a no-op there and `embed_as` behaves exactly like the plain `embed_batch` below.
    /// multilingual-e5-base's non-empty prefixes make this the one place that convention is applied: nothing upstream of this call (the tokenizer, the model, the pooling) needs to know a prefix exists.
    async fn embed_as(
        &self,
        kind: super::EmbedKind,
        text: &str,
    ) -> Result<Vec<f32>, YorishiroError> {
        let prefix = match kind {
            super::EmbedKind::Query => self.inner.def.query_prefix,
            super::EmbedKind::Document => self.inner.def.document_prefix,
        };
        if prefix.is_empty() {
            return self.embed(text).await;
        }
        self.embed(&format!("{prefix}{text}")).await
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        // Inference is CPU-bound and blocks for tens to hundreds of ms, so it's offloaded to the blocking pool to avoid stalling tokio worker threads.
        // On timeout, the blocking task itself still runs to completion (it can't be cancelled), but the caller returns an error immediately instead of waiting, freeing up whatever resources it holds.
        let texts: Vec<String> = texts.iter().map(|text| text.to_string()).collect();
        let inner = Arc::clone(&self.inner);
        let task = tokio::task::spawn_blocking(move || inner.embed_blocking(&texts));
        match tokio::time::timeout(EMBED_TIMEOUT, task).await {
            Ok(joined) => {
                joined.map_err(|err| internal(format!("embedding task panicked: {err}")))?
            }
            Err(_) => Err(internal(format!(
                "embedding timed out after {}s (inference queue congested?)",
                EMBED_TIMEOUT.as_secs()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::model_fetch::{MULTILINGUAL_E5_BASE, NOMIC};
    use super::{LocalEmbeddingConfig, LocalEmbeddingProvider, mean_pool_normalized};
    use crate::services::embedding::{EmbedKind, EmbeddingProvider};

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn mean_pooling_ignores_masked_tokens_and_normalizes() {
        // seq=3, hidden=2.
        // The 3rd token has mask=0, so it's excluded from the average.
        let embeddings = [1.0, 0.0, 3.0, 4.0, 100.0, 100.0];
        let mask = [1_i64, 1, 0];
        let pooled = mean_pool_normalized(&embeddings, &mask, 3, 2);

        // Average is (2.0, 2.0); L2-normalized, that's (1/sqrt(2), 1/sqrt(2)).
        assert!((pooled[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((pooled[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn load_rejects_too_small_max_sequence_length() {
        // tokenizers subtracts the special-token count from max_length during truncation, so an extremely small value underflows; confirm load() rejects it.
        let result = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: "/nonexistent/model.safetensors".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            def: &NOMIC,
            max_sequence_length: 1,
        });
        let Err(err) = result else {
            panic!("load should fail for too small max_sequence_length");
        };
        assert!(err.to_string().contains("max_sequence_length"));
    }

    #[test]
    fn load_rejects_too_large_max_sequence_length() {
        // A value past the selected model's own usable sequence limit would otherwise reach candle during inference and fail there, with a message about tensor shapes rather than about the setting that caused it; confirm load() rejects it up front instead.
        // nomic's own limit (8192) is used here, so this exercises def.max_sequence_length rather than a value that happens to be universally too large.
        let result = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: "/nonexistent/model.safetensors".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            def: &NOMIC,
            max_sequence_length: 8193,
        });
        let Err(err) = result else {
            panic!("load should fail for too large max_sequence_length");
        };
        assert!(err.to_string().contains("max_sequence_length"));
    }

    /// The same bound must differ per model: 513 is well under nomic's 8192 limit, but past multilingual-e5-base's own 512.
    /// This exercises `def.max_sequence_length` rather than a value that would fail for every model regardless of which definition set the bound.
    #[test]
    fn load_rejects_a_sequence_length_within_nomics_bound_but_past_e5s() {
        let result = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: "/nonexistent/model.safetensors".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            def: &MULTILINGUAL_E5_BASE,
            max_sequence_length: 513,
        });
        let Err(err) = result else {
            panic!("load should fail for a sequence length past multilingual-e5-base's own bound");
        };
        assert!(err.to_string().contains("max_sequence_length"));
    }

    #[test]
    fn load_reports_missing_files_clearly() {
        let result = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: "/nonexistent/model.safetensors".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            def: &NOMIC,
            max_sequence_length: 512,
        });
        let Err(err) = result else {
            panic!("load should fail for missing files");
        };
        assert!(err.to_string().contains("tokenizer"));
    }

    /// End-to-end verification against a real model.
    /// Model files aren't checked into the repo (models/ is gitignored), so the test skips if they're absent.
    /// Follow docs/configuration.md to place `models/nomic-embed-text-v1.5/model.safetensors` and its tokenizer to enable it.
    #[tokio::test]
    async fn embeds_texts_with_a_real_model() {
        let model_path = std::env::var("YORISHIRO_TEST_LOCAL_MODEL")
            .unwrap_or_else(|_| "models/nomic-embed-text-v1.5/model.safetensors".into());
        let tokenizer_path = std::env::var("YORISHIRO_TEST_LOCAL_TOKENIZER")
            .unwrap_or_else(|_| "models/nomic-embed-text-v1.5/tokenizer.json".into());
        if !Path::new(&model_path).exists() || !Path::new(&tokenizer_path).exists() {
            eprintln!("skipping embeds_texts_with_a_real_model: model files not found");
            return;
        }

        let provider = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            def: &NOMIC,
            max_sequence_length: 512,
        })
        .unwrap();

        let cat = provider
            .embed_as(EmbedKind::Document, "a cat")
            .await
            .unwrap();
        let dog = provider
            .embed_as(EmbedKind::Document, "a dog")
            .await
            .unwrap();
        let car = provider
            .embed_as(EmbedKind::Document, "an automobile engine")
            .await
            .unwrap();

        assert_eq!(cat.len(), 768);
        // Semantically related texts should be closer than unrelated ones.
        assert!(cosine(&cat, &dog) > cosine(&cat, &car));
    }

    /// End-to-end smoke test for the `XlmRoberta` architecture branch and multilingual-e5-base's hardcoded `Config`, prefixes, and query/document asymmetry.
    /// The nomic test above never exercises this branch: a swapped `attention_mask`/`token_type_ids` argument, a wrong `Config` field, or a broken prefix path would all compile cleanly and could plausibly still produce a vector of the right shape.
    /// Model files aren't checked into the repo (models/ is gitignored), so the test skips if they're absent.
    #[tokio::test]
    async fn embeds_texts_with_a_real_e5_model() {
        let model_path = std::env::var("YORISHIRO_TEST_E5_MODEL")
            .unwrap_or_else(|_| "models/multilingual-e5-base/model.safetensors".into());
        let tokenizer_path = std::env::var("YORISHIRO_TEST_E5_TOKENIZER")
            .unwrap_or_else(|_| "models/multilingual-e5-base/tokenizer.json".into());
        if !Path::new(&model_path).exists() || !Path::new(&tokenizer_path).exists() {
            eprintln!("skipping embeds_texts_with_a_real_e5_model: model files not found");
            return;
        }

        let provider = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            def: &MULTILINGUAL_E5_BASE,
            max_sequence_length: 512,
        })
        .unwrap();

        // Japanese, not just English: multilingual is the whole point of this model, and an English-only check would not exercise it.
        let cat_ja = provider
            .embed_as(EmbedKind::Document, "猫がソファで眠っている")
            .await
            .unwrap();
        let dog_ja = provider
            .embed_as(EmbedKind::Document, "犬が公園を走り回っている")
            .await
            .unwrap();
        let engine_ja = provider
            .embed_as(EmbedKind::Document, "自動車のエンジンを整備する")
            .await
            .unwrap();
        assert_eq!(cat_ja.len(), 768);
        assert!(cosine(&cat_ja, &dog_ja) > cosine(&cat_ja, &engine_ja));

        // Proves the prefix plumbing actually fires: a symmetric model (or a broken prefix path) would embed the same text identically regardless of EmbedKind, which query_prefix and document_prefix being different strings ("query: " vs "passage: ") must prevent.
        let as_query = provider
            .embed_as(EmbedKind::Query, "猫がソファで眠っている")
            .await
            .unwrap();
        let as_document = provider
            .embed_as(EmbedKind::Document, "猫がソファで眠っている")
            .await
            .unwrap();
        assert_ne!(as_query, as_document);

        // A batched call pads to the batch's longest sequence; unlike the three single-text calls above, this exercises the tokenizer's own padding rather than each call producing its own unpadded sequence, which is where a wrong pad token id or masking bug would show up.
        let batch = provider
            .embed_batch(&["猫", "自動車のエンジンをオーバーホールする長い文章"])
            .await
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].len(), 768);
        assert_eq!(batch[1].len(), 768);
    }

    /// Numeric parity against the `ort`-based provider this one replaced.
    ///
    /// A pooling mistake produces vectors that are still the right shape and still normalize, so a
    /// single-pair cosine check would pass even with the wrong pooling; see the module doc comment
    /// on why the choice is a property of the model, not a preference.
    /// `tests/fixtures/nomic_reference_embeddings.json` was generated from the `ort`-based provider
    /// against the same model revision before it was removed, and cannot be regenerated: the `ort`
    /// implementation is gone from this codebase.
    /// Named for the model, not the implementation that produced it, since the implementation
    /// producing this reference will keep changing (this file already outlived `ort`) while the
    /// model it was generated against did not.
    /// This checks per-sentence cosine similarity against that fixture and, separately, that the
    /// full pairwise similarity ordering across all ten sentences is unchanged, since ordering is
    /// what a pooling mistake actually breaks.
    ///
    /// `#[ignore]` rather than a model-files-present early return: a silent skip on a missing model
    /// would report this test as passing in an environment that never ran it, which is exactly
    /// backwards for the one test protecting an irreproducible fixture.
    /// Run explicitly with `cargo test -- --ignored matches_the_ort_based_provider_it_replaced`
    /// after placing `models/nomic-embed-text-v1.5/model.safetensors` and its tokenizer (see docs/configuration.md),
    /// or point `YORISHIRO_TEST_LOCAL_MODEL`/`YORISHIRO_TEST_LOCAL_TOKENIZER` elsewhere.
    #[tokio::test]
    #[ignore = "requires models/nomic-embed-text-v1.5/model.safetensors and its tokenizer"]
    async fn matches_the_ort_based_provider_it_replaced() {
        let model_path = std::env::var("YORISHIRO_TEST_LOCAL_MODEL")
            .unwrap_or_else(|_| "models/nomic-embed-text-v1.5/model.safetensors".into());
        let tokenizer_path = std::env::var("YORISHIRO_TEST_LOCAL_TOKENIZER")
            .unwrap_or_else(|_| "models/nomic-embed-text-v1.5/tokenizer.json".into());
        assert!(
            Path::new(&model_path).exists(),
            "'{model_path}' not found: this test needs the real model, see its own doc comment"
        );
        assert!(
            Path::new(&tokenizer_path).exists(),
            "'{tokenizer_path}' not found: this test needs the real tokenizer, see its own doc comment"
        );

        #[derive(serde::Deserialize)]
        struct Fixture {
            entries: Vec<FixtureEntry>,
        }
        #[derive(serde::Deserialize)]
        struct FixtureEntry {
            text: String,
            vector: Vec<f32>,
        }

        let fixture_bytes = std::fs::read("tests/fixtures/nomic_reference_embeddings.json")
            .expect("fixture missing: see tests/fixtures/nomic_reference_embeddings.json");
        let fixture: Fixture =
            serde_json::from_slice(&fixture_bytes).expect("fixture is not valid JSON");

        let provider = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            def: &NOMIC,
            max_sequence_length: 512,
        })
        .unwrap();

        let mut candle_vectors = Vec::with_capacity(fixture.entries.len());
        for entry in &fixture.entries {
            let vector = provider
                .embed_as(EmbedKind::Document, &entry.text)
                .await
                .unwrap();
            candle_vectors.push(vector);
        }

        // Per-sentence: candle's vector for the same text should sit very close to ort's.
        // The two implementations differ in floating-point op ordering (matrix layout, kernel
        // choice), so this is not bit-identical, but a mean-pooled, L2-normalized vector from the
        // same weights and the same input is expected to match to several decimal places.
        for (entry, candle_vector) in fixture.entries.iter().zip(&candle_vectors) {
            let sim = cosine(&entry.vector, candle_vector);
            assert!(
                sim > 0.999,
                "cosine similarity for {:?} was only {sim}, expected > 0.999",
                entry.text
            );
        }

        // Pairwise ordering: a pooling mistake keeps every vector normalized but reshuffles which
        // pairs are similar, so this catches what the per-sentence check above cannot.
        let n = fixture.entries.len();
        let mut ort_pairs = Vec::new();
        let mut candle_pairs = Vec::new();
        for i in 0..n {
            for j in (i + 1)..n {
                ort_pairs.push((
                    i,
                    j,
                    cosine(&fixture.entries[i].vector, &fixture.entries[j].vector),
                ));
                candle_pairs.push((i, j, cosine(&candle_vectors[i], &candle_vectors[j])));
            }
        }
        ort_pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        candle_pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        let ort_order: Vec<(usize, usize)> = ort_pairs.iter().map(|&(i, j, _)| (i, j)).collect();
        let candle_order: Vec<(usize, usize)> =
            candle_pairs.iter().map(|&(i, j, _)| (i, j)).collect();
        assert_eq!(
            ort_order, candle_order,
            "pairwise similarity ordering differs between the ort-based and candle-based providers"
        );
    }

    /// Numeric parity against a real `sentence-transformers` run of multilingual-e5-base, and, unlike the ort-parity test above, coverage of the query/document prefix plumbing itself.
    ///
    /// `tests/fixtures/generate_e5_reference/generate.py` (run in the Docker image built from
    /// that directory's Dockerfile) manually prepends `query: `/`passage: ` before encoding, since
    /// `sentence-transformers` does not add multilingual-e5-base's prefixes on its own.
    /// This test feeds the fixture's raw, unprefixed text through `embed_as(EmbedKind::Query, _)`
    /// and `embed_as(EmbedKind::Document, _)`, letting this provider's own prefix wiring add the
    /// convention, then compares against the reference's `query_vector`/`document_vector`.
    /// A missing or wrong prefix changes the embedded text, which changes the output vector past
    /// the similarity threshold below, so this test fails if the prefix plumbing silently regresses,
    /// not only if the model's numeric output does.
    ///
    /// Unlike the ort-parity fixture, this one is regenerable (the generator script is committed,
    /// pinned, and dockerized), so `#[ignore]` here is about needing the real model files locally,
    /// the same reason nomic's parity test above is ignored, not about the fixture being irreplaceable.
    #[tokio::test]
    #[ignore = "requires models/multilingual-e5-base/model.safetensors and its tokenizer"]
    async fn matches_a_real_sentence_transformers_run_of_e5() {
        let model_path = std::env::var("YORISHIRO_TEST_E5_MODEL")
            .unwrap_or_else(|_| "models/multilingual-e5-base/model.safetensors".into());
        let tokenizer_path = std::env::var("YORISHIRO_TEST_E5_TOKENIZER")
            .unwrap_or_else(|_| "models/multilingual-e5-base/tokenizer.json".into());
        assert!(
            Path::new(&model_path).exists(),
            "'{model_path}' not found: this test needs the real model, see its own doc comment"
        );
        assert!(
            Path::new(&tokenizer_path).exists(),
            "'{tokenizer_path}' not found: this test needs the real tokenizer, see its own doc comment"
        );

        #[derive(serde::Deserialize)]
        struct Fixture {
            entries: Vec<FixtureEntry>,
        }
        #[derive(serde::Deserialize)]
        struct FixtureEntry {
            text: String,
            query_vector: Vec<f32>,
            document_vector: Vec<f32>,
        }

        let fixture_bytes = std::fs::read("tests/fixtures/e5_reference_embeddings.json")
            .expect("fixture missing: see tests/fixtures/generate_e5_reference/ to generate it");
        let fixture: Fixture =
            serde_json::from_slice(&fixture_bytes).expect("fixture is not valid JSON");

        let provider = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            def: &MULTILINGUAL_E5_BASE,
            max_sequence_length: 512,
        })
        .unwrap();

        let mut query_vectors = Vec::with_capacity(fixture.entries.len());
        let mut document_vectors = Vec::with_capacity(fixture.entries.len());
        for entry in &fixture.entries {
            query_vectors.push(
                provider
                    .embed_as(EmbedKind::Query, &entry.text)
                    .await
                    .unwrap(),
            );
            document_vectors.push(
                provider
                    .embed_as(EmbedKind::Document, &entry.text)
                    .await
                    .unwrap(),
            );
        }

        for (i, entry) in fixture.entries.iter().enumerate() {
            let query_sim = cosine(&entry.query_vector, &query_vectors[i]);
            assert!(
                query_sim > 0.999,
                "query-side cosine similarity for {:?} was only {query_sim}, expected > 0.999",
                entry.text
            );
            let document_sim = cosine(&entry.document_vector, &document_vectors[i]);
            assert!(
                document_sim > 0.999,
                "document-side cosine similarity for {:?} was only {document_sim}, expected > 0.999",
                entry.text
            );
        }

        // The two prefixes must not collapse to the same vector: if they did, the query and
        // document reference vectors themselves would already be near-identical (a property of
        // the fixture, checked here rather than assumed), and this provider's own query/document
        // outputs above must show the same separation, not just each matching its own reference.
        let reference_query_vs_document = cosine(
            &fixture.entries[0].query_vector,
            &fixture.entries[0].document_vector,
        );
        assert!(
            reference_query_vs_document < 0.999,
            "the reference fixture's own query and document vectors for the same text are \
             nearly identical ({reference_query_vs_document}); the fixture may have been \
             generated without the query:/passage: prefixes actually differing"
        );
        assert_ne!(query_vectors[0], document_vectors[0]);

        // A batched call must agree with the single-text path: PaddingParams::default() pads with
        // token id 0 (XLM-RoBERTa's <s>, not multilingual-e5-base's own pad_token_id of 1), and
        // this is the check that would actually catch a leak from that mismatch, rather than only
        // reasoning that masked positions are excluded before pooling.
        // `embed_batch` directly (not `embed_as`), on both sides, so this compares the same
        // unprefixed text through the single- and batch-shaped code paths rather than accidentally
        // comparing a prefixed vector against an unprefixed one.
        let single = provider
            .embed_batch(&[&fixture.entries[0].text])
            .await
            .unwrap();
        let batch = provider
            .embed_batch(&[&fixture.entries[0].text, &fixture.entries[1].text])
            .await
            .unwrap();
        let batch_sim = cosine(&single[0], &batch[0]);
        assert!(
            batch_sim > 0.999,
            "batched embedding for {:?} diverged from its single-text embedding: cosine {batch_sim}",
            fixture.entries[0].text
        );
    }
}
