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
