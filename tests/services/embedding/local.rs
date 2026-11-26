/// Tests for the local embedding provider: mean pooling, config validation, and model loading.
use yorishiro::services::embedding::local::{
    LocalEmbeddingConfig, LocalEmbeddingProvider, mean_pool_normalized,
};
use yorishiro::services::embedding::model_fetch;

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
fn load_rejects_out_of_range_max_sequence_length() {
    for max_sequence_length in [1, 513] {
        let result = LocalEmbeddingProvider::load(&LocalEmbeddingConfig {
            model_path: "/nonexistent/model.safetensors".into(),
            tokenizer_path: "/nonexistent/tokenizer.json".into(),
            def: model_fetch::DEFAULT_MODEL,
            max_sequence_length,
        });
        let Err(err) = result else {
            panic!("load should reject max_sequence_length={max_sequence_length}");
        };
        assert!(err.to_string().contains("max_sequence_length"));
    }
}

#[test]
fn load_reports_missing_files_clearly() {
    let result = LocalEmbeddingProvider::load(&LocalEmbeddingConfig {
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
