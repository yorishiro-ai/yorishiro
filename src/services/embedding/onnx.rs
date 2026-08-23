use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use super::{EmbedKind, EmbeddingProvider};
use crate::error::YorishiroError;

/// Lower bound for `max_sequence_length`.
/// tokenizers subtracts the number of special tokens (2-3 for BERT-family models) from `max_length` during truncation, so a value below that underflows (in release builds this wraps around, silently disabling truncation).
/// There's no practical use for an extremely short sequence length either, so we reject with a comfortable margin.
const MIN_SEQUENCE_LENGTH: usize = 16;

/// Upper bound on wait time for a single embed call.
/// Inference is serialized within the process, so this guards against unbounded waits when prior requests pile up (the local equivalent of the OpenAI-compatible provider's HTTP timeout).
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LocalOnnxConfig {
    pub model_path: PathBuf,
    pub tokenizer_path: PathBuf,
    /// Expected output dimensionality (e.g. 768 for all-mpnet-base-v2).
    /// `load` runs a probe inference and fails startup if the model's actual output dimension doesn't match.
    pub dimensions: usize,
    /// Maximum sequence length for tokenization.
    /// Text longer than this is truncated.
    pub max_sequence_length: usize,
    /// How token embeddings are reduced to one vector.
    /// Must match what the model was trained with; see [`Pooling`].
    pub pooling: Pooling,
    /// Instruction prefixed to search queries, and to queries only.
    /// Qwen3-Embedding expects `Instruct: {task}\nQuery:{text}`; symmetric models want this unset.
    /// `None` embeds queries exactly as documents.
    pub query_instruction: Option<String>,
}

/// Provider that generates embeddings using a local ONNX model (BERT-family encoder).
/// Has no runtime dependency on external services, making it suitable for closed/offline environments.
///
/// Note: because of the `ort` crate's default `download-binaries` feature, **building** this crate downloads the onnxruntime binary from cdn.pyke.io.
/// If even the build environment must be closed off, point `ORT_LIB_LOCATION` at a pre-provisioned onnxruntime instead.
///
/// Model requirements:
/// - Inputs: `input_ids` and `attention_mask` (both int64).
///   `token_type_ids` is only passed if the model declares it.
/// - Output: the first output must be a `[batch, seq, hidden]` last_hidden_state, the shape produced by sentence-transformers ONNX exports.
///
/// Token embeddings are aggregated into a sentence vector via mean pooling weighted by the attention mask, then L2-normalized for stable cosine-distance search.
pub struct LocalOnnxProvider {
    // `Session::run` requires `&mut self`, hence the Mutex for serialization.
    // Inference itself already uses intra-op parallelism across CPU cores, so serializing inference within the process costs little throughput.
    inner: Arc<Inner>,
}

struct Inner {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    dimensions: usize,
    needs_token_type_ids: bool,
    needs_position_ids: bool,
    output_name: String,
    pooling: Pooling,
    query_instruction: Option<String>,
}

fn internal(message: impl std::fmt::Display) -> YorishiroError {
    YorishiroError::Internal(anyhow::anyhow!("{message}"))
}

impl LocalOnnxProvider {
    /// Loads the model and tokenizer from files, validating output dimensionality via a probe inference.
    /// This blocks for hundreds of ms to a few seconds, so call it once at startup only.
    pub fn load(config: LocalOnnxConfig) -> Result<Self, YorishiroError> {
        if config.max_sequence_length < MIN_SEQUENCE_LENGTH {
            return Err(internal(format!(
                "max_sequence_length must be >= {MIN_SEQUENCE_LENGTH}, got {}",
                config.max_sequence_length
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
        tokenizer.with_padding(Some(PaddingParams::default()));

        let builder = Session::builder()
            .map_err(|err| internal(format!("failed to create onnx session builder: {err}")))?;
        let mut builder = builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|err| internal(format!("failed to configure onnx session: {err}")))?;
        let session = builder
            .commit_from_file(&config.model_path)
            .map_err(|err| {
                internal(format!(
                    "failed to load onnx model '{}': {err}",
                    config.model_path.display()
                ))
            })?;

        let needs_token_type_ids = session
            .inputs()
            .iter()
            .any(|outlet| outlet.name() == "token_type_ids");
        // Decoder-style exports (Qwen3-Embedding among them) declare position_ids and fail the whole inference without it, where encoder exports derive positions internally.
        let needs_position_ids = session
            .inputs()
            .iter()
            .any(|outlet| outlet.name() == "position_ids");
        let output_name = session
            .outputs()
            .first()
            .map(|outlet| outlet.name().to_string())
            .ok_or_else(|| internal("onnx model declares no outputs"))?;

        let inner = Inner {
            session: Mutex::new(session),
            tokenizer,
            dimensions: config.dimensions,
            needs_token_type_ids,
            needs_position_ids,
            output_name,
            pooling: config.pooling,
            query_instruction: config.query_instruction,
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
        let mut token_type_ids = Vec::with_capacity(batch * seq);
        for encoding in &encodings {
            input_ids.extend(encoding.get_ids().iter().map(|&v| i64::from(v)));
            attention_mask.extend(encoding.get_attention_mask().iter().map(|&v| i64::from(v)));
            token_type_ids.extend(encoding.get_type_ids().iter().map(|&v| i64::from(v)));
        }

        let shape = vec![batch as i64, seq as i64];
        let to_tensor = |data: Vec<i64>| {
            Tensor::from_array((shape.clone(), data))
                .map_err(|err| internal(format!("failed to build input tensor: {err}")))
        };

        let mut inputs = ort::inputs![
            "input_ids" => to_tensor(input_ids)?,
            "attention_mask" => to_tensor(attention_mask.clone())?,
        ];
        if self.needs_token_type_ids {
            inputs.push(("token_type_ids".into(), to_tensor(token_type_ids)?.into()));
        }
        if self.needs_position_ids {
            // 0..seq per row.
            // Padding is right-side, so every real token sits at the position it would have unpadded and the trailing pad positions are never attended to.
            let position_ids: Vec<i64> = (0..batch).flat_map(|_| 0..seq as i64).collect();
            inputs.push(("position_ids".into(), to_tensor(position_ids)?.into()));
        }

        // Recovers from poisoning: even if a panic occurs while the lock is held, the Session carries no state across inferences (`&mut` is only an artifact of ort's API), so the invariant isn't actually broken.
        // Permanently disabling embedding until a process restart over a poison would cause more harm.
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        let outputs = session
            .run(inputs)
            .map_err(|err| internal(format!("onnx inference failed: {err}")))?;
        let output = outputs
            .get(self.output_name.as_str())
            .ok_or_else(|| internal(format!("onnx output '{}' is missing", self.output_name)))?;
        let (out_shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|err| internal(format!("failed to read onnx output: {err}")))?;

        let dims: &[i64] = out_shape;
        if dims.len() != 3 || dims[0] != batch as i64 || dims[1] != seq as i64 {
            return Err(internal(format!(
                "unexpected onnx output shape {dims:?} (expected [batch={batch}, seq={seq}, hidden])"
            )));
        }
        let hidden = dims[2] as usize;
        if hidden != self.dimensions {
            return Err(internal(format!(
                "onnx model produces {hidden}-dimensional embeddings, expected {}",
                self.dimensions
            )));
        }

        let mut results = Vec::with_capacity(batch);
        for b in 0..batch {
            results.push(pool_normalized(
                self.pooling,
                &data[b * seq * hidden..(b + 1) * seq * hidden],
                &attention_mask[b * seq..(b + 1) * seq],
                seq,
                hidden,
            ));
        }
        Ok(results)
    }
}

/// How token embeddings are reduced to one sentence vector.
///
/// This is a property of the model, not a preference: a model trained with one and read with the other returns vectors that are still the right shape and still normalize, so nothing fails: the search results just quietly get worse.
/// Sentence-transformers exports (BERT-style, bge-small, multilingual-e5) want `Mean`; the Qwen3-Embedding family wants `LastToken`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pooling {
    /// Mean of the unmasked token embeddings.
    #[default]
    Mean,
    /// The last unmasked token's embedding.
    LastToken,
}

impl Pooling {
    /// Parses the `YORISHIRO_ONNX_POOLING` value.
    /// Unknown values are rejected rather than defaulted: silently falling back to `Mean` is exactly the quiet degradation this type exists to prevent.
    pub fn parse(value: &str) -> Result<Self, YorishiroError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mean" => Ok(Self::Mean),
            "last_token" | "last-token" | "lasttoken" => Ok(Self::LastToken),
            other => Err(internal(format!(
                "unknown pooling '{other}': expected 'mean' or 'last_token'"
            ))),
        }
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

/// Takes the last unmasked token's embedding.
/// Padding is right-side for these models, so the last unmasked position is the end of the actual text; falling back to position 0 for an all-masked row keeps the function total.
///
/// `pub` (rather than private) only so this module's own tests can call it directly; `#[doc(hidden)]` keeps it out of the public API docs.
#[doc(hidden)]
pub fn last_token_pool_normalized(
    token_embeddings: &[f32],
    attention_mask: &[i64],
    seq: usize,
    hidden: usize,
) -> Vec<f32> {
    let last = (0..seq)
        .rev()
        .find(|&t| attention_mask[t] != 0)
        .unwrap_or(0);
    let mut pooled = token_embeddings[last * hidden..(last + 1) * hidden].to_vec();
    l2_normalize(&mut pooled);
    pooled
}

/// Reduces token embeddings to one vector using `pooling`.
pub fn pool_normalized(
    pooling: Pooling,
    token_embeddings: &[f32],
    attention_mask: &[i64],
    seq: usize,
    hidden: usize,
) -> Vec<f32> {
    match pooling {
        Pooling::Mean => mean_pool_normalized(token_embeddings, attention_mask, seq, hidden),
        Pooling::LastToken => {
            last_token_pool_normalized(token_embeddings, attention_mask, seq, hidden)
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
impl EmbeddingProvider for LocalOnnxProvider {
    fn dimensions(&self) -> usize {
        self.inner.dimensions
    }

    /// Counted exactly: this provider already holds the tokenizer the model uses, so the number is the one the model will actually see rather than an estimate of it.
    fn count_tokens(&self, text: &str) -> u32 {
        match self.inner.tokenizer.encode(text, false) {
            Ok(encoding) => u32::try_from(encoding.len()).unwrap_or(u32::MAX),
            // A text this tokenizer cannot encode is one the model could not embed either; the request is about to fail on its own, so fall back rather than decide here.
            Err(_) => u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX),
        }
    }

    /// Prefixes the configured instruction to queries, and only to queries.
    /// With no instruction configured this is exactly `embed`, which is what every symmetric model wants.
    async fn embed_as(&self, kind: EmbedKind, text: &str) -> Result<Vec<f32>, YorishiroError> {
        match (kind, self.inner.query_instruction.as_deref()) {
            (EmbedKind::Query, Some(instruction)) => {
                self.embed(&format!("Instruct: {instruction}\nQuery:{text}"))
                    .await
            }
            _ => self.embed(text).await,
        }
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        // ONNX inference is CPU-bound and blocks for tens to hundreds of ms, so it's offloaded to the blocking pool to avoid stalling tokio worker threads.
        // On timeout, the blocking task itself still runs to completion (it can't be cancelled), but the caller returns an error immediately instead of waiting, freeing up whatever resources it holds.
        let texts: Vec<String> = texts.iter().map(|text| text.to_string()).collect();
        let inner = Arc::clone(&self.inner);
        let task = tokio::task::spawn_blocking(move || inner.embed_blocking(&texts));
        match tokio::time::timeout(EMBED_TIMEOUT, task).await {
            Ok(joined) => {
                joined.map_err(|err| internal(format!("embedding task panicked: {err}")))?
            }
            Err(_) => Err(internal(format!(
                "onnx embedding timed out after {}s (inference queue congested?)",
                EMBED_TIMEOUT.as_secs()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        LocalOnnxConfig, LocalOnnxProvider, Pooling, last_token_pool_normalized,
        mean_pool_normalized,
    };
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
    fn last_token_pooling_takes_the_last_unmasked_position() {
        // seq=3, hidden=2. Only the first two tokens are unmasked, so position 1 is "last".
        let embeddings = [1.0, 0.0, 3.0, 4.0, 100.0, 100.0];
        let mask = [1_i64, 1, 0];
        let pooled = last_token_pool_normalized(&embeddings, &mask, 3, 2);

        // Position 1 is (3.0, 4.0); L2-normalized, that's (0.6, 0.8).
        assert!((pooled[0] - 0.6).abs() < 1e-6);
        assert!((pooled[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn pooling_parse_accepts_known_spellings_and_rejects_unknown() {
        assert_eq!(Pooling::parse("mean").unwrap(), Pooling::Mean);
        assert_eq!(Pooling::parse("Mean").unwrap(), Pooling::Mean);
        assert_eq!(Pooling::parse("last_token").unwrap(), Pooling::LastToken);
        assert_eq!(Pooling::parse("last-token").unwrap(), Pooling::LastToken);
        assert_eq!(Pooling::parse("lasttoken").unwrap(), Pooling::LastToken);
        assert!(Pooling::parse("nonsense").is_err());
    }

    #[test]
    fn load_rejects_too_small_max_sequence_length() {
        // tokenizers subtracts the special-token count from max_length during truncation, so an extremely small value underflows; confirm load() rejects it.
        let result = LocalOnnxProvider::load(LocalOnnxConfig {
            model_path: "/nonexistent/model.onnx".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            dimensions: 768,
            max_sequence_length: 1,
            pooling: Default::default(),
            query_instruction: None,
        });
        let Err(err) = result else {
            panic!("load should fail for too small max_sequence_length");
        };
        assert!(err.to_string().contains("max_sequence_length"));
    }

    #[test]
    fn load_reports_missing_files_clearly() {
        let result = LocalOnnxProvider::load(LocalOnnxConfig {
            model_path: "/nonexistent/model.onnx".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            dimensions: 768,
            max_sequence_length: 512,
            pooling: Default::default(),
            query_instruction: None,
        });
        let Err(err) = result else {
            panic!("load should fail for missing files");
        };
        assert!(err.to_string().contains("tokenizer"));
    }

    /// End-to-end verification against a real model.
    /// Model files aren't checked into the repo (models/ is gitignored), so the test skips if they're absent.
    /// Follow docs/setup.md to place models/model.onnx and models/tokenizer.json to enable it.
    #[tokio::test]
    async fn embeds_texts_with_a_real_model() {
        let model_path = std::env::var("YORISHIRO_TEST_ONNX_MODEL")
            .unwrap_or_else(|_| "models/model.onnx".into());
        let tokenizer_path = std::env::var("YORISHIRO_TEST_ONNX_TOKENIZER")
            .unwrap_or_else(|_| "models/tokenizer.json".into());
        if !Path::new(&model_path).exists() || !Path::new(&tokenizer_path).exists() {
            eprintln!("skipping embeds_texts_with_a_real_model: model files not found");
            return;
        }

        let provider = LocalOnnxProvider::load(LocalOnnxConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            dimensions: 768,
            max_sequence_length: 512,
            pooling: Default::default(),
            query_instruction: None,
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
}
