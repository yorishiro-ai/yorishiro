use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::nomic_bert::{Config, NomicBertModel};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use super::EmbeddingProvider;
use crate::error::YorishiroError;

/// Lower bound for `max_sequence_length`.
/// tokenizers subtracts the number of special tokens (2-3 for BERT-family models) from `max_length` during truncation, so a value below that underflows (in release builds this wraps around, silently disabling truncation).
/// There's no practical use for an extremely short sequence length either, so we reject with a comfortable margin.
const MIN_SEQUENCE_LENGTH: usize = 16;

/// Upper bound for `max_sequence_length`, matching `nomic-embed-text-v1.5`'s own `n_positions` (`Config::default().n_positions`, 8192).
/// candle's rotary embedding table is sized to this figure at model load; a longer sequence reaches the rotary embedding during inference and fails there instead of at startup, with a message about tensor shapes rather than about the setting that caused it.
/// This is a property of the one model this provider loads, not a candle limitation, so it belongs on this constant rather than as a general timeout-style config bound.
const MAX_SEQUENCE_LENGTH: usize = 8192;

/// Upper bound on wait time for a single embed call.
/// Inference is serialized within the process, so this guards against unbounded waits when prior requests pile up (the local equivalent of the OpenAI-compatible provider's HTTP timeout).
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

pub struct LocalEmbeddingConfig {
    pub model_path: PathBuf,
    pub tokenizer_path: PathBuf,
    /// Expected output dimensionality (e.g. 768 for nomic-embed-text-v1.5).
    /// `load` runs a probe inference and fails startup if the model's actual output dimension doesn't match.
    pub dimensions: usize,
    /// Maximum sequence length for tokenization.
    /// Text longer than this is truncated.
    pub max_sequence_length: usize,
}

/// Provider that generates embeddings using a local, in-process model.
/// Has no runtime dependency on external services, making it suitable for closed/offline environments.
///
/// The model is `nomic-ai/nomic-embed-text-v1.5` (see `super::model_fetch`), read through `candle-transformers`'
/// `nomic_bert` module from a `safetensors` checkpoint: `NomicBertModel` implements that one architecture, a
/// BERT variant with rotary position embeddings and a SwiGLU MLP, not an arbitrary encoder graph.
/// A different model here is a different codebase, not a config change.
///
/// Token embeddings are aggregated into a sentence vector via mean pooling weighted by the attention mask, then
/// L2-normalized for stable cosine-distance search, matching how nomic-embed-text-v1.5 was trained and evaluated.
pub struct LocalEmbeddingProvider {
    // candle's `Tensor` ops on CPU are not internally parallel across an inference the way onnxruntime's
    // intra-op threading was, but the model is `Send + Sync` regardless; the Mutex here still exists to bound
    // memory: without it, concurrent requests would each build their own batch of activations on a CPU-only
    // process, which is worse than queuing them.
    inner: Arc<Inner>,
}

struct Inner {
    model: Mutex<NomicBertModel>,
    tokenizer: Tokenizer,
    dimensions: usize,
    device: Device,
}

fn internal(message: impl std::fmt::Display) -> YorishiroError {
    YorishiroError::Internal(anyhow::anyhow!("{message}"))
}

impl LocalEmbeddingProvider {
    /// Loads the model and tokenizer from files, validating output dimensionality via a probe inference.
    /// This blocks for hundreds of ms to a few seconds, so call it once at startup only.
    pub fn load(config: LocalEmbeddingConfig) -> Result<Self, YorishiroError> {
        if config.max_sequence_length < MIN_SEQUENCE_LENGTH {
            return Err(internal(format!(
                "max_sequence_length must be >= {MIN_SEQUENCE_LENGTH}, got {}",
                config.max_sequence_length
            )));
        }
        if config.max_sequence_length > MAX_SEQUENCE_LENGTH {
            return Err(internal(format!(
                "max_sequence_length must be <= {MAX_SEQUENCE_LENGTH} (nomic-embed-text-v1.5's own position limit), got {}",
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

        let device = Device::Cpu;
        // `from_buffered_safetensors` (a plain read, not `from_mmaped_safetensors`) is deliberate:
        // the mmap variant is `unsafe` because the caller must guarantee the file is never mutated
        // for as long as the mapping lives, and that guarantee only holds for `model_fetch`'s own
        // managed tier (replaced solely by `rename` into a path this provider has not opened yet).
        // `YORISHIRO_LOCAL_MODEL_PATH` names an operator-chosen path outside that mechanism, with
        // no such guarantee, so an `unsafe` block here would be asserting a safety invariant this
        // function cannot actually promise. The 522 MiB read happens once at startup and is noise
        // next to the model fetch it may follow.
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
        let model = NomicBertModel::load(vb, &Config::default()).map_err(|err| {
            internal(format!(
                "failed to build the nomic-bert model from '{}': {err}",
                config.model_path.display()
            ))
        })?;

        let inner = Inner {
            model: Mutex::new(model),
            tokenizer,
            dimensions: config.dimensions,
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
            .forward(
                &input_ids,
                Some(&token_type_ids),
                Some(&attention_mask_tensor),
            )
            .map_err(|err| internal(format!("model inference failed: {err}")))?;
        drop(model);

        let dims = hidden_states.dims();
        if dims.len() != 3 || dims[0] != batch || dims[1] != seq {
            return Err(internal(format!(
                "unexpected model output shape {dims:?} (expected [batch={batch}, seq={seq}, hidden])"
            )));
        }
        let hidden = dims[2];
        if hidden != self.dimensions {
            return Err(internal(format!(
                "model produces {hidden}-dimensional embeddings, expected {}",
                self.dimensions
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
            dimensions: 768,
            max_sequence_length: 1,
        });
        let Err(err) = result else {
            panic!("load should fail for too small max_sequence_length");
        };
        assert!(err.to_string().contains("max_sequence_length"));
    }

    #[test]
    fn load_rejects_too_large_max_sequence_length() {
        // A value past NomicBertModel's own rotary embedding table would otherwise reach candle
        // during inference and fail there, with a message about tensor shapes rather than about
        // the setting that caused it; confirm load() rejects it up front instead.
        let result = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: "/nonexistent/model.safetensors".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            dimensions: 768,
            max_sequence_length: 8193,
        });
        let Err(err) = result else {
            panic!("load should fail for too large max_sequence_length");
        };
        assert!(err.to_string().contains("max_sequence_length"));
    }

    #[test]
    fn load_reports_missing_files_clearly() {
        let result = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: "/nonexistent/model.safetensors".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            dimensions: 768,
            max_sequence_length: 512,
        });
        let Err(err) = result else {
            panic!("load should fail for missing files");
        };
        assert!(err.to_string().contains("tokenizer"));
    }

    /// End-to-end verification against a real model.
    /// Model files aren't checked into the repo (models/ is gitignored), so the test skips if they're absent.
    /// Follow docs/setup.md to place models/model.safetensors and models/tokenizer.json to enable it.
    #[tokio::test]
    async fn embeds_texts_with_a_real_model() {
        let model_path = std::env::var("YORISHIRO_TEST_LOCAL_MODEL")
            .unwrap_or_else(|_| "models/model.safetensors".into());
        let tokenizer_path = std::env::var("YORISHIRO_TEST_LOCAL_TOKENIZER")
            .unwrap_or_else(|_| "models/tokenizer.json".into());
        if !Path::new(&model_path).exists() || !Path::new(&tokenizer_path).exists() {
            eprintln!("skipping embeds_texts_with_a_real_model: model files not found");
            return;
        }

        let provider = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            dimensions: 768,
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

    /// Numeric parity against the `ort`-based provider this one replaced.
    ///
    /// A pooling mistake produces vectors that are still the right shape and still normalize, so a
    /// single-pair cosine check would pass even with the wrong pooling; see the module doc comment
    /// on why the choice is a property of the model, not a preference.
    /// `tests/fixtures/onnx_reference_embeddings.json` was generated from the `ort`-based provider
    /// against the same model revision before it was removed, and cannot be regenerated: the `ort`
    /// implementation is gone from this codebase.
    /// This checks per-sentence cosine similarity against that fixture and, separately, that the
    /// full pairwise similarity ordering across all ten sentences is unchanged, since ordering is
    /// what a pooling mistake actually breaks.
    ///
    /// `#[ignore]` rather than a model-files-present early return: a silent skip on a missing model
    /// would report this test as passing in an environment that never ran it, which is exactly
    /// backwards for the one test protecting an irreproducible fixture.
    /// Run explicitly with `cargo test -- --ignored matches_the_ort_based_provider_it_replaced`
    /// after placing `models/model.safetensors` and `models/tokenizer.json` (see docs/configuration.md),
    /// or point `YORISHIRO_TEST_LOCAL_MODEL`/`YORISHIRO_TEST_LOCAL_TOKENIZER` elsewhere.
    #[tokio::test]
    #[ignore = "requires models/model.safetensors and models/tokenizer.json"]
    async fn matches_the_ort_based_provider_it_replaced() {
        let model_path = std::env::var("YORISHIRO_TEST_LOCAL_MODEL")
            .unwrap_or_else(|_| "models/model.safetensors".into());
        let tokenizer_path = std::env::var("YORISHIRO_TEST_LOCAL_TOKENIZER")
            .unwrap_or_else(|_| "models/tokenizer.json".into());
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

        let fixture_bytes = std::fs::read("tests/fixtures/onnx_reference_embeddings.json")
            .expect("fixture missing: see tests/fixtures/onnx_reference_embeddings.json");
        let fixture: Fixture =
            serde_json::from_slice(&fixture_bytes).expect("fixture is not valid JSON");

        let provider = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            dimensions: 768,
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
}
