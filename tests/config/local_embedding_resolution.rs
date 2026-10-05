//! Tests for local embedding provider resolution without loading real model files.
//!
//! These tests verify that the embedding provider resolution path works correctly.

use loco_rs::{app::Hooks, boot::StartMode, environment::Environment};

/// When `YORISHIRO_EMBEDDING_PROVIDER=local` is configured with a non-existent
/// model name, boot fails with a clear error.  This proves the provider does not
/// silently install a no-op — unknown models are caught at boot.
/// ServerOnly is used because the provider is built unconditionally in all modes;
/// the old ServerAndScheduler path is the only other mode and it too builds the
/// provider, so ServerOnly exercises the same path at lower cost.
#[tokio::test]
async fn unknown_local_model_name_fails_at_boot() {
    let config =
        serde_yaml::from_str::<loco_rs::config::Config>(&minimal_config_with_unknown_model(
            "sqlite:///tmp/test_unknown_model.sqlite3?mode=rwc",
            "nonexistent-model-that-does-not-exist",
        ))
        .unwrap();

    let result = yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config).await;

    match result {
        Ok(_) => panic!("unknown model name must fail at boot"),
        Err(e) => {
            let err_str = format!("{e}");
            assert!(
                err_str.contains("not a known local model"),
                "error must mention unknown model name: {err_str}"
            );
        }
    }
}

/// When `YORISHIRO_EMBEDDING_PROVIDER=none`, the provider is explicitly disabled
/// and `UnconfiguredEmbeddingProvider` is installed with a clear remedy message.
#[tokio::test]
async fn none_provider_installs_noop_with_clear_message() {
    let config = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config_with_none_embedding("sqlite:///tmp/test_none.sqlite3?mode=rwc"),
    )
    .unwrap();

    let result = yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config).await;

    match result {
        Ok(boot) => {
            let ctx = boot.app_context;
            let provider = ctx
                .shared_store
                .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
                .expect("embedding provider must be installed");

            assert_eq!(
                provider.model_name(),
                "unconfigured",
                "none provider must install no-op"
            );

            let err = provider.embed("test").await.unwrap_err();
            let err_str = format!("{err}");
            assert!(
                err_str.contains("YORISHIRO_EMBEDDING_PROVIDER is set to \"none\""),
                "none provider error must mention provider=none: {err_str}"
            );
        }
        Err(e) => {
            panic!("none provider boot should succeed: {e}");
        }
    }
}

fn minimal_config_with_unknown_model(uri: &str, model_name: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: sqlite://queue.sqlite3?mode=rwc\n  num_workers: 1\nworkers:\n  mode: BackgroundQueue\nsettings:\n  max_tenants: 1\n  embedding:\n    provider: local\n    dimensions: 768\n    local_model: {model_name}\n    api_key: \"\"\n    send_dimensions_param: false\n    local_max_sequence_length: 512\n  rate_limit:\n    auth_max_requests: 10\n    auth_window_seconds: 60\n    search_tokens_per_minute: 100000\n  db_load_guard:\n    threshold: 0\n    sustain_seconds: 30\n    poll_seconds: 5\n"
    )
}

fn minimal_config_with_none_embedding(uri: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: sqlite://queue.sqlite3?mode=rwc\n  num_workers: 1\nworkers:\n  mode: BackgroundQueue\nsettings:\n  max_tenants: 1\n  embedding:\n    provider: none\n    dimensions: 768\n    api_key: \"\"\n    send_dimensions_param: false\n    local_model: multilingual-e5-base\n    local_max_sequence_length: 512\n  rate_limit:\n    auth_max_requests: 10\n    auth_window_seconds: 60\n    search_tokens_per_minute: 100000\n  db_load_guard:\n    threshold: 0\n    sustain_seconds: 30\n    poll_seconds: 5\n"
    )
}
