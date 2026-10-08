use yorishiro::data::settings::{EmbeddingProvider, Settings};

fn settings() -> serde_json::Value {
    serde_json::json!({
        "max_tenants": 1,
        "embedding": {
            "provider": "local",
            "dimensions": 768,
            "base_url": null,
            "api_key": "",
            "model": null,
            "send_dimensions_param": false,
            "local_model": "multilingual-e5-base",
            "local_max_sequence_length": 512
        },
        "rate_limit": {
            "auth_max_requests": 10,
            "auth_window_seconds": 60,
            "search_tokens_per_minute": 100000
        },
        "db_load_guard": {
            "threshold": 0,
            "sustain_seconds": 30,
            "poll_seconds": 5
        }
    })
}

#[test]
fn provider_is_typed_and_unknown_settings_are_rejected() {
    let parsed: Settings = serde_json::from_value(settings()).unwrap();
    assert_eq!(parsed.embedding.provider, EmbeddingProvider::Local);

    let mut unknown_provider = settings();
    unknown_provider["embedding"]["provider"] = "typo".into();
    assert!(serde_json::from_value::<Settings>(unknown_provider).is_err());

    let mut unknown_key = settings();
    unknown_key["rate_limit"]["obsolete"] = 1.into();
    assert!(serde_json::from_value::<Settings>(unknown_key).is_err());
}

#[test]
fn query_embedding_section_is_optional_and_has_bounded_defaults() {
    use yorishiro::data::settings::QueryEmbedding;

    let parsed: Settings = serde_json::from_value(settings()).unwrap();
    assert_eq!(parsed.query_embedding, QueryEmbedding::default());
    assert_eq!(parsed.query_embedding.timeout_ms, 15_000);
    assert_eq!(parsed.query_embedding.poll_interval_ms, 100);
    assert_eq!(parsed.query_embedding.retention_seconds, 300);

    let mut tuned = settings();
    tuned["query_embedding"] = serde_json::json!({"timeout_ms": 2000});
    let parsed: Settings = serde_json::from_value(tuned).unwrap();
    assert_eq!(parsed.query_embedding.timeout_ms, 2000);
    assert_eq!(parsed.query_embedding.poll_interval_ms, 100);

    let mut unknown = settings();
    unknown["query_embedding"] = serde_json::json!({"obsolete": 1});
    assert!(serde_json::from_value::<Settings>(unknown).is_err());
}
