//! Which process roles build the embedding provider.
//!
//! The provider can hold a model of a gigabyte or more, so only a process that consumes the queue builds it.
//! The API and MCP server, the scheduler, and every CLI command run without one.
//! These tests boot with the real `Hooks` and a configuration that would make provider construction fail loudly, so a role that built one would fail instead of passing.

use std::sync::Arc;

use loco_rs::{app::Hooks, boot::StartMode, environment::Environment};
use yorishiro::services::embedding::EmbeddingProvider;

const UNKNOWN_MODEL: &str = "nonexistent-model-that-does-not-exist";

fn provider_of(ctx: &loco_rs::app::AppContext) -> Option<Arc<dyn EmbeddingProvider>> {
    ctx.shared_store.get::<Arc<dyn EmbeddingProvider>>()
}

fn unknown_model_config(dir: &tempfile::TempDir) -> loco_rs::config::Config {
    let (uri, queue_uri) = sqlite_uris(dir);
    serde_yaml::from_str(&minimal_config_with_unknown_model(
        &uri,
        &queue_uri,
        UNKNOWN_MODEL,
    ))
    .unwrap()
}

/// The server boots with a local provider that could never load, because it never builds one, and its liveness probe does not depend on a model.
#[tokio::test]
async fn the_server_boots_without_building_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let boot = yorishiro::App::boot(
        StartMode::ServerOnly,
        &Environment::Test,
        unknown_model_config(&dir),
    )
    .await
    .expect("a server must not need the embedding model");

    assert!(provider_of(&boot.app_context).is_none());

    let router = boot.router.expect("a server builds a router");
    let server = axum_test::TestServer::new(
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .unwrap();
    for probe in ["/_ping", "/_health"] {
        assert_eq!(server.get(probe).await.status_code(), 200, "{probe}");
    }
}

/// `create_context` is what every CLI command, the scheduler and the first of `start`'s two contexts run, so nothing built after it can be built by them.
#[tokio::test]
async fn a_context_alone_never_builds_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = loco_rs::boot::create_context::<yorishiro::App>(
        &Environment::Test,
        unknown_model_config(&dir),
    )
    .await
    .expect("a bare context must not need the embedding model");

    assert!(provider_of(&ctx).is_none());
}

/// A worker builds the provider at start, so an unknown model fails the worker loudly instead of failing its first job.
#[tokio::test]
async fn a_worker_with_an_unknown_local_model_fails_at_start() {
    for mode in [
        StartMode::WorkerOnly { tags: vec![] },
        StartMode::ServerAndWorker,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let result =
            yorishiro::App::boot(mode, &Environment::Test, unknown_model_config(&dir)).await;
        match result {
            Ok(_) => panic!("unknown model name must fail a worker's start"),
            Err(e) => {
                let err_str = format!("{e}");
                assert!(
                    err_str.contains("not a known local model"),
                    "error must mention unknown model name: {err_str}"
                );
            }
        }
    }
}

/// With `YORISHIRO_EMBEDDING_PROVIDER=none` the worker holds the explicitly disabled provider with a clear remedy, and connecting its workers again keeps that one instance.
#[tokio::test]
async fn a_worker_holds_exactly_one_provider_and_none_means_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, queue_uri) = sqlite_uris(&dir);
    let config = serde_yaml::from_str::<loco_rs::config::Config>(
        &minimal_config_with_none_embedding(&uri, &queue_uri),
    )
    .unwrap();

    let boot = yorishiro::App::boot(
        StartMode::WorkerOnly { tags: vec![] },
        &Environment::Test,
        config,
    )
    .await
    .expect("none provider boot should succeed");
    let ctx = boot.app_context;
    let provider = provider_of(&ctx).expect("a worker installs its provider");

    assert_eq!(provider.model_name(), "unconfigured");
    let err = provider.embed("test").await.unwrap_err();
    assert!(
        format!("{err}").contains("YORISHIRO_EMBEDDING_PROVIDER is set to \"none\""),
        "none provider error must mention provider=none: {err}"
    );

    let queue = ctx.queue_provider.clone().expect("queue");
    yorishiro::App::connect_workers(&ctx, &queue)
        .await
        .expect("connecting again");
    assert!(
        Arc::ptr_eq(&provider, &provider_of(&ctx).unwrap()),
        "a second connect keeps the single instance"
    );
}

/// Application and queue databases private to one test, so parallel tests share no file.
fn sqlite_uris(dir: &tempfile::TempDir) -> (String, String) {
    let uri = |name: &str| format!("sqlite://{}?mode=rwc", dir.path().join(name).display());
    (uri("app.sqlite3"), uri("queue.sqlite3"))
}

fn minimal_config_with_unknown_model(uri: &str, queue_uri: &str, model_name: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: {queue_uri}\n  num_workers: 1\nworkers:\n  mode: BackgroundQueue\nsettings:\n  max_tenants: 1\n  embedding:\n    provider: local\n    dimensions: 768\n    local_model: {model_name}\n    api_key: \"\"\n    send_dimensions_param: false\n    local_max_sequence_length: 512\n  rate_limit:\n    auth_max_requests: 10\n    auth_window_seconds: 60\n    search_tokens_per_minute: 100000\n  db_load_guard:\n    threshold: 0\n    sustain_seconds: 30\n    poll_seconds: 5\n"
    )
}

fn minimal_config_with_none_embedding(uri: &str, queue_uri: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: {queue_uri}\n  num_workers: 1\nworkers:\n  mode: BackgroundQueue\nsettings:\n  max_tenants: 1\n  embedding:\n    provider: none\n    dimensions: 768\n    api_key: \"\"\n    send_dimensions_param: false\n    local_model: multilingual-e5-base\n    local_max_sequence_length: 512\n  rate_limit:\n    auth_max_requests: 10\n    auth_window_seconds: 60\n    search_tokens_per_minute: 100000\n  db_load_guard:\n    threshold: 0\n    sustain_seconds: 30\n    poll_seconds: 5\n"
    )
}
