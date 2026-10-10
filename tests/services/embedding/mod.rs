mod model_fetch;
mod openai;
use serial_test::serial;

use yorishiro::data::settings::Settings;
use yorishiro::services::embedding::build_embedding_provider;

use crate::EnvGuard;

/// Every retired variable and, for a renamed one, the name that replaced it.
const RETIRED_VARS: [(&str, Option<&str>); 7] = [
    (
        "YORISHIRO_ONNX_MODEL_PATH",
        Some("YORISHIRO_LOCAL_MODEL_PATH"),
    ),
    (
        "YORISHIRO_ONNX_TOKENIZER_PATH",
        Some("YORISHIRO_LOCAL_TOKENIZER_PATH"),
    ),
    ("YORISHIRO_LOCAL_MODEL_PATH", None),
    ("YORISHIRO_LOCAL_TOKENIZER_PATH", None),
    (
        "YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH",
        Some("YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH"),
    ),
    ("YORISHIRO_ONNX_POOLING", None),
    ("YORISHIRO_ONNX_QUERY_INSTRUCTION", None),
];

fn local_settings(local_model: &str, dimensions: usize) -> Settings {
    serde_json::from_value(serde_json::json!({
        "max_tenants": 1,
        "embedding": {
            "provider": "local", "dimensions": dimensions, "base_url": null, "api_key": "",
            "model": null, "send_dimensions_param": false,
            "local_model": local_model, "local_max_sequence_length": 512
        },
        "rate_limit": {
            "auth_max_requests": 10, "auth_window_seconds": 60, "search_tokens_per_minute": 100000
        },
        "db_load_guard": { "threshold": 0, "sustain_seconds": 30, "poll_seconds": 5 }
    }))
    .expect("settings")
}

async fn local_error(settings: &Settings) -> String {
    match build_embedding_provider(settings).await {
        Ok(_) => panic!("the local provider must refuse this configuration"),
        Err(error) => error.to_string(),
    }
}

fn clear_retired(guard: &EnvGuard) {
    for (old, _) in RETIRED_VARS {
        guard.remove(old);
    }
}

fn retired_names() -> Vec<&'static str> {
    RETIRED_VARS.iter().map(|(old, _)| *old).collect()
}

/// Boot must stop on any retired variable, naming it and, when one exists, its replacement.
/// The models are never loaded: the guard runs before any file or network access.
#[tokio::test]
#[serial(process_environment)]
async fn a_retired_variable_stops_the_local_provider_and_names_its_replacement() {
    let guard = EnvGuard::capture(&retired_names());
    let settings = local_settings("multilingual-e5-base", 768);
    for (old, replacement) in RETIRED_VARS {
        clear_retired(&guard);
        guard.set(old, "x");
        let message = local_error(&settings).await;
        assert!(message.contains(old), "{old}: {message}");
        if let Some(replacement) = replacement {
            assert!(message.contains(replacement), "{old}: {message}");
        }
    }
}

/// `std::env::var` reports a non-UTF-8 value as an error, the same as an unset variable, so the guard has to look at presence alone.
#[cfg(unix)]
#[tokio::test]
#[serial(process_environment)]
async fn a_retired_variable_with_a_non_utf8_value_is_still_refused() {
    use std::os::unix::ffi::OsStringExt;

    let guard = EnvGuard::capture(&retired_names());
    clear_retired(&guard);
    let old = "YORISHIRO_ONNX_MODEL_PATH";
    guard.set(old, std::ffi::OsString::from_vec(vec![0xFF, 0xFE, 0xFD]));

    let message = local_error(&local_settings("multilingual-e5-base", 768)).await;

    assert!(message.contains(old), "{message}");
}

/// With no retired variable set the guard passes, so the next check is what answers.
#[tokio::test]
#[serial(process_environment)]
async fn an_unknown_or_removed_local_model_is_refused_at_boot() {
    let guard = EnvGuard::capture(&retired_names());
    clear_retired(&guard);

    let unknown = local_error(&local_settings("typo-model", 768)).await;
    assert!(unknown.contains("typo-model"), "{unknown}");
    assert!(unknown.contains("multilingual-e5-base"), "{unknown}");

    let removed = local_error(&local_settings("nomic-embed-text-v1.5", 768)).await;
    assert!(removed.contains("has been removed"), "{removed}");
}

#[tokio::test]
#[serial(process_environment)]
async fn a_dimension_the_selected_model_does_not_produce_is_refused_at_boot() {
    let guard = EnvGuard::capture(&retired_names());
    clear_retired(&guard);

    let message = local_error(&local_settings("multilingual-e5-base", 1024)).await;

    assert!(message.contains("1024"), "{message}");
}
