use std::sync::Arc;

use anyhow::Result;
use yorishiro_core::services::embedding::onnx::{LocalOnnxConfig, LocalOnnxProvider};
use yorishiro_core::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

pub mod admin;
mod error;
mod http;
pub mod logging;
mod routes;
mod state;

pub use routes::{apply_observability_layers, build_app};
pub use state::AppState;

/// `YORISHIRO_MAX_TENANTS` is process-wide state read by both `http::controllers::setup` and login's
/// workspace auto-resolution, so every test across the crate that sets it (rather than just
/// asserting the default) must serialize through this one shared lock -- a per-module lock
/// only prevents that module's own tests from racing each other, not tests in a different
/// module running concurrently in the same `cargo test` process.
#[cfg(test)]
pub(crate) mod max_tenants_env_lock {
    pub static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub fn set(value: Option<&str>) {
        match value {
            Some(v) => unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", v) },
            None => unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") },
        }
    }
}

/// Starts a graceful shutdown on Ctrl-C (all platforms) or SIGTERM (Unix only,
/// the standard stop signal from container orchestrators).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received, draining connections");
}

/// Builds the embeddings provider from environment variables. `YSR_EMBEDDING_PROVIDER`
/// switches between `local` (a local ONNX model, the default -- needs no external service or
/// API key, just the model files under `models/`) and `openai` (an OpenAI-compatible API, for
/// operators already running something like Ollama/LM Studio). The `entities.embedding`
/// column is `vector` (dimensionless), so any model works; all vectors in a deployment must
/// share the same dimension count (set via `YSR_EMBEDDING_DIMENSIONS`, default 768).
pub fn build_embedding_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let dimensions: usize = std::env::var("YSR_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "768".into())
        .parse()?;

    let kind = std::env::var("YSR_EMBEDDING_PROVIDER").unwrap_or_else(|_| "local".into());
    match kind.as_str() {
        "openai" => {
            let base_url = std::env::var("YSR_EMBEDDING_BASE_URL").map_err(|_| {
                anyhow::anyhow!(
                    "YSR_EMBEDDING_BASE_URL must be set when YSR_EMBEDDING_PROVIDER=openai"
                )
            })?;
            let model = std::env::var("YSR_EMBEDDING_MODEL").map_err(|_| {
                anyhow::anyhow!(
                    "YSR_EMBEDDING_MODEL must be set when YSR_EMBEDDING_PROVIDER=openai"
                )
            })?;
            let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: base_url.clone(),
                api_key: std::env::var("YSR_EMBEDDING_API_KEY").unwrap_or_default(),
                model: model.clone(),
                dimensions,
                send_dimensions_param: std::env::var("YSR_EMBEDDING_SEND_DIMENSIONS_PARAM")
                    .map(|v| v == "true")
                    .unwrap_or(true),
            });
            tracing::info!(provider = "openai", %base_url, %model, dimensions, "embedding provider configured");
            Ok(Arc::new(provider))
        }
        "local" => {
            let max_sequence_length: usize = std::env::var("YSR_ONNX_MAX_SEQUENCE_LENGTH")
                .unwrap_or_else(|_| "512".into())
                .parse()?;
            let model_path =
                std::env::var("YSR_ONNX_MODEL_PATH").unwrap_or_else(|_| "models/model.onnx".into());
            let tokenizer_path = std::env::var("YSR_ONNX_TOKENIZER_PATH")
                .unwrap_or_else(|_| "models/tokenizer.json".into());
            let provider = LocalOnnxProvider::load(LocalOnnxConfig {
                model_path: model_path.clone().into(),
                tokenizer_path: tokenizer_path.into(),
                dimensions,
                max_sequence_length,
            })?;
            tracing::info!(provider = "local", %model_path, dimensions, "embedding provider configured");
            Ok(Arc::new(provider))
        }
        other => {
            anyhow::bail!("unknown YSR_EMBEDDING_PROVIDER '{other}' (expected 'openai' or 'local')")
        }
    }
}

#[cfg(test)]
mod tests;
