//! Optional `config.yml` support. This binary has always been configured entirely through
//! environment variables (see `.env.example`); this module lets an operator put the same
//! settings in a YAML file instead, without changing how any of them are actually consumed.
//!
//! It does this by reading the file (if present) and, for each setting it sets, writing the
//! corresponding environment variable -- but only if that variable isn't already set. Every
//! existing `std::env::var("YSR_...")` call site elsewhere in this crate and in
//! `yorishiro-core` is untouched: environment variables still win when both are set, and a
//! deployment with no `config.yml` behaves exactly as before.
//!
//! This is only ever invoked from this crate's `main` (in its synchronous prologue, before the
//! tokio runtime starts -- see `load_and_apply_env_overrides`'s safety contract). It lives here
//! rather than in `main.rs` so `tests/config.rs` can reach it as an ordinary public item. A
//! downstream binary that embeds this crate's library API directly, rather than going through
//! this binary's `main`, simply never calls it.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    database_url: Option<String>,
    bind: Option<String>,
    web_dir: Option<String>,
    cors_origins: Option<String>,
    max_tenants: Option<i64>,
    rust_log: Option<String>,
    #[serde(default)]
    embedding: EmbeddingConfig,
    #[serde(default)]
    logging: LoggingConfig,
    #[serde(default)]
    auth_rate_limit: AuthRateLimitConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EmbeddingConfig {
    provider: Option<String>,
    dimensions: Option<u32>,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    send_dimensions_param: Option<bool>,
    onnx_model_path: Option<String>,
    onnx_tokenizer_path: Option<String>,
    onnx_max_sequence_length: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LoggingConfig {
    target: Option<String>,
    dir: Option<String>,
    syslog_socket: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AuthRateLimitConfig {
    max: Option<u32>,
    window_secs: Option<u64>,
}

/// Sets `key` to `value` unless it's already set in the environment.
///
/// # Safety
///
/// Must only be called from a synchronous prologue in `main`, before the tokio runtime (or any
/// other thread) starts and before anything else reads or writes the environment -- `set_var`
/// is unsound under concurrent env access, which this ordering rules out.
unsafe fn apply_if_unset(key: &str, value: Option<String>) {
    if let Some(value) = value
        && std::env::var_os(key).is_none()
    {
        unsafe { std::env::set_var(key, value) };
    }
}

/// Loads `config.yml` (path overridable via `YSR_CONFIG_PATH`, defaulting to `config.yml` in
/// the working directory) and materializes its settings into the process environment. A
/// missing file is not an error -- it just means every setting stays exactly as the
/// environment already has it, which is the same as if this function were never called.
///
/// # Safety
///
/// See `apply_if_unset`: must be called from `main`'s synchronous prologue, before the tokio
/// runtime starts.
pub unsafe fn load_and_apply_env_overrides() -> Result<()> {
    let path = std::env::var("YSR_CONFIG_PATH").unwrap_or_else(|_| "config.yml".into());
    let path = Path::new(&path);
    if !path.exists() {
        return Ok(());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file '{}'", path.display()))?;
    let config: FileConfig = serde_yaml_ng::from_str(&contents)
        .with_context(|| format!("failed to parse config file '{}'", path.display()))?;

    // SAFETY: forwarded from this function's own contract.
    unsafe {
        apply_if_unset("DATABASE_URL", config.database_url);
        apply_if_unset("YSR_BIND", config.bind);
        apply_if_unset("YSR_WEB_DIR", config.web_dir);
        apply_if_unset("YSR_CORS_ORIGINS", config.cors_origins);
        apply_if_unset(
            "YORISHIRO_MAX_TENANTS",
            config.max_tenants.map(|n| n.to_string()),
        );
        apply_if_unset("RUST_LOG", config.rust_log);

        apply_if_unset("YSR_EMBEDDING_PROVIDER", config.embedding.provider);
        apply_if_unset(
            "YSR_EMBEDDING_DIMENSIONS",
            config.embedding.dimensions.map(|n| n.to_string()),
        );
        apply_if_unset("YSR_EMBEDDING_BASE_URL", config.embedding.base_url);
        apply_if_unset("YSR_EMBEDDING_MODEL", config.embedding.model);
        apply_if_unset("YSR_EMBEDDING_API_KEY", config.embedding.api_key);
        apply_if_unset(
            "YSR_EMBEDDING_SEND_DIMENSIONS_PARAM",
            config
                .embedding
                .send_dimensions_param
                .map(|b| b.to_string()),
        );
        apply_if_unset("YSR_ONNX_MODEL_PATH", config.embedding.onnx_model_path);
        apply_if_unset(
            "YSR_ONNX_TOKENIZER_PATH",
            config.embedding.onnx_tokenizer_path,
        );
        apply_if_unset(
            "YSR_ONNX_MAX_SEQUENCE_LENGTH",
            config
                .embedding
                .onnx_max_sequence_length
                .map(|n| n.to_string()),
        );

        apply_if_unset("YSR_LOG_TARGET", config.logging.target);
        apply_if_unset("YSR_LOG_DIR", config.logging.dir);
        apply_if_unset("YSR_SYSLOG_SOCKET", config.logging.syslog_socket);

        apply_if_unset(
            "YSR_AUTH_RATE_LIMIT_MAX",
            config.auth_rate_limit.max.map(|n| n.to_string()),
        );
        apply_if_unset(
            "YSR_AUTH_RATE_LIMIT_WINDOW_SECS",
            config.auth_rate_limit.window_secs.map(|n| n.to_string()),
        );
    }

    Ok(())
}
