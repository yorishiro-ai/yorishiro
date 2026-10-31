mod local;
mod model_fetch;
mod openai;
use serial_test::serial;

use yorishiro::data::settings::Settings;
use yorishiro::services::embedding::build_embedding_provider;

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

#[tokio::test]
#[serial(process_environment)]
async fn an_unknown_local_model_is_refused_at_boot() {
    let unknown = local_error(&local_settings("typo-model", 768)).await;
    assert!(unknown.contains("typo-model"), "{unknown}");
    assert!(unknown.contains("multilingual-e5-base"), "{unknown}");
}

#[tokio::test]
#[serial(process_environment)]
async fn a_dimension_the_selected_model_does_not_produce_is_refused_at_boot() {
    let message = local_error(&local_settings("multilingual-e5-base", 1024)).await;

    assert!(message.contains("1024"), "{message}");
}
