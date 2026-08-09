use std::path::Path;

use crate::services::embedding::EmbeddingProvider;
use crate::services::embedding::onnx::{LocalOnnxConfig, LocalOnnxProvider, mean_pool_normalized};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[test]
fn mean_pooling_ignores_masked_tokens_and_normalizes() {
    // seq=3, hidden=2. The 3rd token has mask=0, so it's excluded from the average.
    let embeddings = [1.0, 0.0, 3.0, 4.0, 100.0, 100.0];
    let mask = [1_i64, 1, 0];
    let pooled = mean_pool_normalized(&embeddings, &mask, 3, 2);

    // Average is (2.0, 2.0); L2-normalized, that's (1/√2, 1/√2).
    assert!((pooled[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!((pooled[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
}

#[test]
fn load_rejects_too_small_max_sequence_length() {
    // tokenizers subtracts the special-token count from max_length during
    // truncation, so an extremely small value underflows; confirm load() rejects it.
    let result = LocalOnnxProvider::load(LocalOnnxConfig {
        model_path: "/nonexistent/model.onnx".into(),
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
fn load_reports_missing_files_clearly() {
    let result = LocalOnnxProvider::load(LocalOnnxConfig {
        model_path: "/nonexistent/model.onnx".into(),
        tokenizer_path: "/nonexistent/tokenizer.json".into(),
        dimensions: 768,
        max_sequence_length: 512,
    });
    let Err(err) = result else {
        panic!("load should fail for missing files");
    };
    assert!(err.to_string().contains("tokenizer"));
}

/// End-to-end verification against a real model. Model files aren't
/// checked into the repo (models/ is gitignored), so the test skips if
/// they're absent. Follow the README to place models/model.onnx and
/// models/tokenizer.json to enable it.
#[tokio::test]
async fn embeds_texts_with_a_real_model() {
    let model_path =
        std::env::var("YSR_TEST_ONNX_MODEL").unwrap_or_else(|_| "../../models/model.onnx".into());
    let tokenizer_path = std::env::var("YSR_TEST_ONNX_TOKENIZER")
        .unwrap_or_else(|_| "../../models/tokenizer.json".into());
    if !Path::new(&model_path).exists() || !Path::new(&tokenizer_path).exists() {
        eprintln!("skipping embeds_texts_with_a_real_model: model files not found");
        return;
    }

    let provider = LocalOnnxProvider::load(LocalOnnxConfig {
        model_path: model_path.into(),
        tokenizer_path: tokenizer_path.into(),
        dimensions: 768,
        max_sequence_length: 512,
    })
    .unwrap();
    assert_eq!(provider.dimensions(), 768);

    let vectors = provider
        .embed_batch(&[
            "The weather is lovely and sunny today.",
            "It is a beautiful clear day outside.",
            "PostgreSQL row level security policies",
        ])
        .await
        .unwrap();

    assert_eq!(vectors.len(), 3);
    for vector in &vectors {
        assert_eq!(vector.len(), 768);
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "vector must be L2-normalized");
    }

    // Two semantically similar sentences should have higher cosine similarity than an unrelated one.
    let same_topic = cosine(&vectors[0], &vectors[1]);
    let different_topic = cosine(&vectors[0], &vectors[2]);
    assert!(
        same_topic > different_topic,
        "similar sentences should be closer: {same_topic} vs {different_topic}"
    );
}
