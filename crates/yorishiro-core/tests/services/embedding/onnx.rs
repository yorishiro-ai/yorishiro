use std::path::Path;

use crate::services::embedding::onnx::{
    LocalOnnxConfig, LocalOnnxProvider, Pooling, last_token_pool_normalized, mean_pool_normalized,
};
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

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
        pooling: Default::default(),
        query_instruction: None,
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

#[test]
fn last_token_pooling_takes_the_final_unmasked_position() {
    // Three positions, two dimensions. The third is padding, so the second is the last real
    // token and the one whose embedding should come back.
    let embeddings = vec![
        1.0, 0.0, // t0
        0.0, 2.0, // t1  <- last unmasked
        9.0, 9.0, // t2  (padding)
    ];
    let mask = vec![1, 1, 0];

    let pooled = last_token_pool_normalized(&embeddings, &mask, 3, 2);

    // t1 normalized: (0, 2) -> (0, 1). The padding row must not contribute.
    assert!((pooled[0] - 0.0).abs() < 1e-6, "got {pooled:?}");
    assert!((pooled[1] - 1.0).abs() < 1e-6, "got {pooled:?}");
}

/// The two poolings must actually differ, otherwise selecting one would be meaningless.
#[test]
fn mean_and_last_token_pooling_disagree() {
    let embeddings = vec![1.0, 0.0, 0.0, 1.0];
    let mask = vec![1, 1];

    let mean = mean_pool_normalized(&embeddings, &mask, 2, 2);
    let last = last_token_pool_normalized(&embeddings, &mask, 2, 2);

    assert!(
        (mean[0] - last[0]).abs() > 0.1,
        "mean {mean:?} and last-token {last:?} should not coincide"
    );
}

#[test]
fn last_token_pooling_survives_an_all_masked_row() {
    let embeddings = vec![3.0, 4.0];
    let mask = vec![0];

    // Falls back to position 0 rather than panicking on an empty iterator.
    let pooled = last_token_pool_normalized(&embeddings, &mask, 1, 2);
    assert!((pooled[0] - 0.6).abs() < 1e-6, "got {pooled:?}");
}

#[test]
fn pooling_parses_its_accepted_spellings() {
    assert_eq!(Pooling::parse("mean").unwrap(), Pooling::Mean);
    assert_eq!(Pooling::parse("last_token").unwrap(), Pooling::LastToken);
    assert_eq!(Pooling::parse("  LAST-TOKEN ").unwrap(), Pooling::LastToken);
    assert_eq!(Pooling::default(), Pooling::Mean);
}

/// An unknown value must fail rather than fall back to the default: silently pooling a
/// last-token model with the mean is the failure this setting exists to prevent, and it
/// produces no error of its own.
#[test]
fn pooling_rejects_an_unknown_value() {
    let err = Pooling::parse("cls").unwrap_err();
    assert!(
        format!("{err}").contains("expected 'mean' or 'last_token'"),
        "got {err}"
    );
}

/// With no instruction configured, a query and a document embed identically — this is the
/// path every symmetric model takes, including the current default.
#[tokio::test]
async fn without_an_instruction_queries_and_documents_agree() {
    let Some(provider) = real_model_provider(None) else {
        return;
    };

    let as_query = provider
        .embed_as(EmbedKind::Query, "shopping list")
        .await
        .unwrap();
    let as_document = provider
        .embed_as(EmbedKind::Document, "shopping list")
        .await
        .unwrap();

    assert_eq!(as_query, as_document);
}

/// With one configured, the query diverges and the document does not. Asserting the document
/// is untouched is the half that matters: prefixing stored text too would reintroduce the
/// symmetry this exists to break.
#[tokio::test]
async fn an_instruction_changes_queries_only() {
    let Some(provider) = real_model_provider(Some("Retrieve relevant documents")) else {
        return;
    };

    let plain = provider.embed("shopping list").await.unwrap();
    let as_document = provider
        .embed_as(EmbedKind::Document, "shopping list")
        .await
        .unwrap();
    let as_query = provider
        .embed_as(EmbedKind::Query, "shopping list")
        .await
        .unwrap();

    assert_eq!(as_document, plain, "documents are embedded verbatim");
    assert!(
        cosine(&as_query, &plain) < 0.999,
        "the query should not embed identically to the bare text"
    );
}

fn real_model_provider(instruction: Option<&str>) -> Option<LocalOnnxProvider> {
    let model_path =
        std::env::var("YSR_TEST_ONNX_MODEL").unwrap_or_else(|_| "../../models/model.onnx".into());
    let tokenizer_path = std::env::var("YSR_TEST_ONNX_TOKENIZER")
        .unwrap_or_else(|_| "../../models/tokenizer.json".into());
    if !Path::new(&model_path).exists() || !Path::new(&tokenizer_path).exists() {
        eprintln!("skipping: model files not found");
        return None;
    }
    Some(
        LocalOnnxProvider::load(LocalOnnxConfig {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            dimensions: 768,
            max_sequence_length: 512,
            pooling: Default::default(),
            query_instruction: instruction.map(str::to_string),
        })
        .unwrap(),
    )
}
