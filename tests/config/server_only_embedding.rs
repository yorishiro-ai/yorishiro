//! Tests for `ServerOnly` mode embedding provider behavior.
//!
//! In `ServerOnly` mode the local embedding model must NOT be loaded, even when
//! `YORISHIRO_EMBEDDING_PROVIDER=local` is configured.  The provider should be a
//! no-op that errors on actual embed calls with a helpful message.

use loco_rs::{app::Hooks, boot::StartMode, environment::Environment};
use tokio::join;

use super::{minimal_config_with_local_embedding, minimal_config_with_openai_embedding};

/// `ServerOnly` mode must NOT fail when local embedding provider is configured but
/// model files are absent.  Without the fix, boot fails because `build_local_provider`
/// tries to load or fetch the model files.  With the fix, boot succeeds and installs a
/// no-op provider.
#[tokio::test]
async fn server_only_boot_succeeds_without_local_embedding_model_files() {
    // This test runs from the repo root where `models/` does not contain the
    // multilingual-e5-base files, so the local provider would normally fail.
    // ServerOnly mode should skip loading it.
    let config = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config_with_local_embedding("sqlite://./test_server_only.sqlite3?mode=rwc"),
    )
    .unwrap();

    let result = yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config).await;
    assert!(
        result.is_ok(),
        "ServerOnly boot must not fail due to missing local embedding model files"
    );
}

/// `ServerOnly` mode must install an `UnconfiguredEmbeddingProvider` (no-op)
/// rather than the real local provider.  The no-op errors on every embed call
/// with a message that explains how to enable embeddings.
#[tokio::test]
async fn server_only_installs_noop_embedding_provider() {
    let config =
        serde_yaml::from_str::<loco_rs::config::Config>(&minimal_config_with_local_embedding(
            "sqlite://./test_server_only_provider.sqlite3?mode=rwc",
        ))
        .unwrap();

    let boot_result =
        yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config).await;
    assert!(boot_result.is_ok(), "ServerOnly boot must succeed");

    let ctx = boot_result.unwrap().app_context;
    let provider = ctx
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
        .expect("embedding provider must be installed in shared_store");

    // The no-op provider's model_name must be "unconfigured".
    assert_eq!(
        provider.model_name(),
        "unconfigured",
        "ServerOnly mode must install the no-op embedding provider"
    );
}

/// `ServerOnly` mode no-op provider must error on embed with a helpful remedy.
#[tokio::test]
async fn server_only_noop_provider_errors_on_embed() {
    let config = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config_with_local_embedding("sqlite://./test_server_only_embed.sqlite3?mode=rwc"),
    )
    .unwrap();

    let boot_result =
        yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config).await;
    assert!(boot_result.is_ok(), "ServerOnly boot must succeed");

    let ctx = boot_result.unwrap().app_context;
    let provider = ctx
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
        .expect("embedding provider must be installed");

    // The no-op provider must error on any embed call.
    let err = provider.embed("test text").await.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("server-only mode"),
        "no-op embed error must mention server-only mode: {err_str}"
    );
    assert!(
        err_str.contains("YORISHIRO_EMBEDDING_BASE_URL"),
        "no-op embed error must suggest YORISHIRO_EMBEDDING_BASE_URL: {err_str}"
    );
}

/// Sequential `ServerOnly` then `WorkerOnly` must not leak state: the second
/// boot must NOT install the no-op provider (proving the task-local scope
/// is per-boot, not process-global).
#[tokio::test]
async fn sequential_server_only_then_worker_only_no_state_leakage() {
    let config = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config_with_local_embedding("sqlite://./test_seq.sqlite3?mode=rwc"),
    )
    .unwrap();

    // First: ServerOnly succeeds and installs a no-op provider.
    let result1 =
        yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config.clone()).await;
    assert!(result1.is_ok(), "First boot (ServerOnly) must succeed");
    let ctx1 = result1.unwrap().app_context;
    let provider1 = ctx1
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
        .unwrap();
    assert_eq!(
        provider1.model_name(),
        "unconfigured",
        "ServerOnly must install the no-op provider"
    );

    // Second: WorkerOnly must install a REAL provider (not the no-op).
    // Even if model files are cached, the provider model_name must differ
    // from "unconfigured".
    let result2 = yorishiro::app::App::boot(
        StartMode::WorkerOnly {
            tags: vec!["worker-class:shared".to_owned()],
        },
        &Environment::Test,
        config.clone(),
    )
    .await;
    assert!(
        result2.is_ok(),
        "WorkerOnly may succeed if model files are cached; the key invariant is that \
         it installs a real provider, not the no-op"
    );

    let ctx2 = result2.unwrap().app_context;
    let provider2 = ctx2
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
        .expect("embedding provider must be installed");
    assert_ne!(
        provider2.model_name(),
        "unconfigured",
        "WorkerOnly must install a real provider, not the ServerOnly no-op. \
         This proves the task-local scope does not leak across boots."
    );
}

/// `ServerOnly` mode with an OpenAI-compatible provider (base_url + model set)
/// must still install the configured remote provider.  Only the local provider
/// is skipped.
#[tokio::test]
async fn server_only_with_openai_provider_installs_remote_provider() {
    let config =
        serde_yaml::from_str::<loco_rs::config::Config>(&minimal_config_with_openai_embedding(
            "sqlite://./test_openai_server_only.sqlite3?mode=rwc",
        ))
        .unwrap();

    let result = yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config).await;
    assert!(
        result.is_ok(),
        "ServerOnly boot must succeed with OpenAI provider"
    );

    let ctx = result.unwrap().app_context;
    let provider = ctx
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
        .expect("embedding provider must be installed");

    // OpenAI-compatible provider has its own model_name (not "unconfigured").
    assert!(
        provider.model_name() != "unconfigured",
        "ServerOnly with OpenAI provider must install the real remote provider, not the no-op"
    );
}

/// Concurrent `ServerOnly` (local provider) and `ServerOnly` (OpenAI provider)
/// boots must be independent: local installs a no-op, OpenAI installs the
/// remote provider.  This proves the task-local scope isolates each boot from
/// the other, even when they run on the same Tokio runtime.
#[tokio::test]
async fn concurrent_server_only_boot_isolation() {
    let config_local = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config_with_local_embedding("sqlite://./test_concurrent_local.sqlite3?mode=rwc"),
    )
    .unwrap();
    let config_openai = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config_with_openai_embedding("sqlite://./test_concurrent_openai.sqlite3?mode=rwc"),
    )
    .unwrap();

    let (result_local, result_openai) = join!(
        yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config_local),
        yorishiro::app::App::boot(StartMode::ServerOnly, &Environment::Test, config_openai),
    );

    // Local provider ServerOnly must install no-op.
    assert!(result_local.is_ok());
    let local_ctx = result_local.unwrap().app_context;
    let local_provider = local_ctx
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
        .unwrap();
    assert_eq!(
        local_provider.model_name(),
        "unconfigured",
        "Concurrent ServerOnly with local provider must install the no-op"
    );

    // OpenAI provider ServerOnly must install the real remote provider.
    assert!(result_openai.is_ok());
    let openai_ctx = result_openai.unwrap().app_context;
    let openai_provider = openai_ctx
        .shared_store
        .get::<std::sync::Arc<dyn yorishiro::services::embedding::EmbeddingProvider>>()
        .unwrap();
    assert_ne!(
        openai_provider.model_name(),
        "unconfigured",
        "Concurrent ServerOnly with OpenAI provider must install the real \
         remote provider, proving the task-local scope does not leak."
    );
}
