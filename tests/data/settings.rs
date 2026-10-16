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
fn timezone_defaults_to_utc_and_accepts_strict_iana_names() {
    let parsed: Settings = serde_json::from_value(settings()).unwrap();
    assert_eq!(parsed.timezone, "UTC");

    let mut tokyo = settings();
    tokyo["timezone"] = "Asia/Tokyo".into();
    assert_eq!(
        serde_json::from_value::<Settings>(tokyo).unwrap().timezone,
        "Asia/Tokyo"
    );

    let mut invalid = settings();
    invalid["timezone"] = "Not/A_Zone".into();
    let invalid: Settings = serde_json::from_value(invalid).unwrap();
    assert!(yorishiro::time::parse_timezone(&invalid.timezone).is_err());
}

#[test]
fn config_validation_rejects_invalid_timezone_and_accepts_valid_aliases() {
    for valid in ["UTC", "Etc/UTC", "Asia/Tokyo", "America/New_York"] {
        assert!(yorishiro::data::config::validate(&config_for_timezone(valid)).is_ok());
    }
    for invalid in ["local", "PST", "+09:00", "Not/A_Zone"] {
        assert!(yorishiro::data::config::validate(&config_for_timezone(invalid)).is_err());
    }
}

fn config_for_timezone(timezone: &str) -> loco_rs::config::Config {
    use loco_rs::{config::Config, environment::Environment};
    use std::{fs, path::PathBuf};

    let dir = std::env::temp_dir().join(format!("timezone-config-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&dir).unwrap();
    let source = include_str!("../../config/test_sqlite.yaml").replace(
        "<%= get_env(name=\"YORISHIRO_TIMEZONE\", default=\"UTC\") %>",
        timezone,
    );
    fs::write(dir.join("test.yaml"), source).unwrap();
    let config =
        Config::from_folder(&Environment::Any("test".into()), &PathBuf::from(&dir)).unwrap();
    fs::remove_dir_all(dir).unwrap();
    config
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
