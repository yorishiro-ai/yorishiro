use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::{Activation, VarBuilder};
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

/// One of the `candle-transformers` model families this provider can load, selected by [`Architecture`].
enum LoadedModel {
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
            Self::Xlm(model) => {
                model.forward(input_ids, attention_mask, token_type_ids, None, None, None)
            }
        }
    }
}

/// `intfloat/multilingual-e5-base`'s `config.json` at revision `d128750597153bb5987e10b1c3493a34e5a4502a`, transcribed field-by-field rather than fetched: `xlm_roberta::Config` has no `Default` impl, and this avoids a third pinned-and-digested artifact for a file that never changes independently of the revision already pinned on [`super::model_fetch::DEFAULT_MODEL`].
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
/// A field with one legal value on every definition so far would encode a future constraint on the field; one arrives the day a model that pools differently does.
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

    /// Prepends the selected model's query/document prefix before embedding.
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
