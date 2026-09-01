/// Tests for the local embedding provider: mean pooling, config validation, and model loading.
use std::path::Path;

use yorishiro::services::embedding::{EmbedKind, EmbeddingProvider};
use yorishiro::services::embedding::local::{LocalEmbeddingConfig, LocalEmbeddingProvider, mean_pool_normalized};
use yorishiro::services::embedding::model_fetch;

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
        def: model_fetch::DEFAULT_MODEL,
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
    let result = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
        model_path: "/nonexistent/model.safetensors".into(),
        tokenizer_path: "/nonexistent/tokenizer.json".into(),
        def: model_fetch::DEFAULT_MODEL,
        max_sequence_length: 513,
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
        def: model_fetch::DEFAULT_MODEL,
        max_sequence_length: 512,
    });
    let Err(err) = result else {
        panic!("load should fail for missing files");
    };
    assert!(err.to_string().contains("tokenizer"));
}

/// End-to-end smoke test for the `XlmRoberta` architecture branch and multilingual-e5-base's hardcoded `Config`, prefixes, and query/document asymmetry.
/// A swapped `attention_mask`/`token_type_ids` argument, a wrong `Config` field, or a broken prefix path would all compile cleanly and could plausibly still produce a vector of the right shape.
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
        def: model_fetch::DEFAULT_MODEL,
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

    let fixture_bytes = std::fs::read("tests/fixtures/multilingual-e5-base_reference_embeddings.json")
        .expect("fixture missing: see tests/fixtures/generate_e5_reference/ to generate it");
    let fixture: Fixture =
        serde_json::from_slice(&fixture_bytes).expect("fixture is not valid JSON");

    let provider = LocalEmbeddingProvider::load(LocalEmbeddingConfig {
        model_path: model_path.into(),
        tokenizer_path: tokenizer_path.into(),
        def: model_fetch::DEFAULT_MODEL,
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
