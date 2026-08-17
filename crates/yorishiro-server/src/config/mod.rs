//! Optional `config.yml` support: an operator may put settings in a YAML file instead of the environment.
//!
//! The file is read if present, and each setting it names is written to the corresponding environment variable, but only where that variable is unset.
//! Every `std::env::var` call site reads the environment as usual, so an environment variable wins over the file.
//!
//! Invoked from the binary's synchronous prologue, before the tokio runtime starts: see `load_and_apply_env_overrides`'s safety contract.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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
    #[serde(default)]
    db_load_guard: DbLoadGuardConfig,
    search_tokens_per_minute: Option<u32>,
    snapshot_retention_days: Option<i32>,
    /// Accepted and ignored here; `ee/` reads it.
    /// The key has to exist in this struct because it is `deny_unknown_fields` and both editions parse the same file: without it, a config carrying `license_key:` would refuse to start on the community build.
    ///
    /// Deliberately not applied to the environment from here.
    /// Doing that would put the string `YORISHIRO_LICENSE_KEY` in the community binary, which the release gate scans for and would reject: correctly, since that binary is meant to carry no trace of the paid edition.
    #[allow(dead_code)]
    license_key: Option<String>,
}

/// The load shedder's settings.
/// Present here because they are documented, and this struct is `deny_unknown_fields`: a documented key that has no field makes a copied example config refuse to start.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DbLoadGuardConfig {
    threshold: Option<u32>,
    poll_secs: Option<u64>,
    sustain_secs: Option<u64>,
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
    onnx_pooling: Option<String>,
    onnx_query_instruction: Option<String>,
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
/// other thread) starts and before anything else reads or writes the environment: `set_var`
/// is unsound under concurrent env access, which this ordering rules out.
unsafe fn apply_if_unset(key: &str, value: Option<String>) {
    if let Some(value) = value
        && std::env::var_os(key).is_none()
    {
        unsafe { std::env::set_var(key, value) };
    }
}

/// Where a package install puts the configuration, and where the unit points `YORISHIRO_CONFIG_PATH` at.
pub const PACKAGED_CONFIG_PATH: &str = "/etc/yorishiro/config.yml";

/// Which file to read, or `None` when there is nothing to read.
///
/// `YORISHIRO_CONFIG_PATH` wins outright when it is set: an operator naming a file means that file, and silently reading a different one would be worse than reading none.
///
/// Unset, the working directory comes first, so a source checkout keeps using its own `config.yml`.
/// `/etc/yorishiro/config.yml` is the fallback, and it exists for the admin CLI:
/// the unit exports the variable, but a shell has no such environment, so without this fallback `yorishiro-server admin create-tenant` on a packaged host would look in whatever directory the operator happens to be in and report the database as unconfigured, even while the service beside it runs normally against that exact file.
///
/// `explicit` is an `OsString` rather than a `String` because `std::env::var` reports a non-UTF-8 value as `NotUnicode`, and `.ok()` would flatten that into "unset".
/// A path this process cannot render as UTF-8 is still a path the operator named, and treating it as unset would fall back to a different file: the one case this function exists to rule out.
///
/// Split out as a pure function so the precedence is testable without touching the process environment or the filesystem root.
pub(crate) fn config_path_from(
    explicit: Option<OsString>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(named) = explicit {
        let named = PathBuf::from(named);
        return exists(&named).then_some(named);
    }
    let local = PathBuf::from("config.yml");
    if exists(&local) {
        return Some(local);
    }
    let packaged = PathBuf::from(PACKAGED_CONFIG_PATH);
    exists(&packaged).then_some(packaged)
}

/// Loads `config.yml` and materializes its settings into the process environment. A missing file
/// is not an error: it means every setting stays exactly as the environment already has it,
/// which is the same as if this function were never called.
///
/// See [`config_path_from`] for which file is read.
///
/// # Safety
///
/// See `apply_if_unset`: must be called from `main`'s synchronous prologue, before the tokio
/// runtime starts.
pub unsafe fn load_and_apply_env_overrides() -> Result<()> {
    let explicit = std::env::var_os("YORISHIRO_CONFIG_PATH");
    let Some(path) = config_path_from(explicit, |p| p.exists()) else {
        return Ok(());
    };
    let path = path.as_path();

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file '{}'", path.display()))?;
    let config: FileConfig = serde_yaml_ng::from_str(&contents)
        .with_context(|| format!("failed to parse config file '{}'", path.display()))?;

    // SAFETY: forwarded from this function's own contract.
    unsafe {
        apply_if_unset("DATABASE_URL", config.database_url);
        apply_if_unset("YORISHIRO_BIND", config.bind);
        apply_if_unset("YORISHIRO_WEB_DIR", config.web_dir);
        apply_if_unset("YORISHIRO_CORS_ORIGINS", config.cors_origins);
        apply_if_unset(
            "YORISHIRO_MAX_TENANTS",
            config.max_tenants.map(|n| n.to_string()),
        );
        apply_if_unset("RUST_LOG", config.rust_log);

        apply_if_unset("YORISHIRO_EMBEDDING_PROVIDER", config.embedding.provider);
        apply_if_unset(
            "YORISHIRO_EMBEDDING_DIMENSIONS",
            config.embedding.dimensions.map(|n| n.to_string()),
        );
        apply_if_unset("YORISHIRO_EMBEDDING_BASE_URL", config.embedding.base_url);
        apply_if_unset("YORISHIRO_EMBEDDING_MODEL", config.embedding.model);
        apply_if_unset("YORISHIRO_EMBEDDING_API_KEY", config.embedding.api_key);
        apply_if_unset(
            "YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM",
            config
                .embedding
                .send_dimensions_param
                .map(|b| b.to_string()),
        );
        apply_if_unset(
            "YORISHIRO_ONNX_MODEL_PATH",
            config.embedding.onnx_model_path,
        );
        apply_if_unset("YORISHIRO_ONNX_POOLING", config.embedding.onnx_pooling);
        apply_if_unset(
            "YORISHIRO_ONNX_QUERY_INSTRUCTION",
            config.embedding.onnx_query_instruction,
        );
        apply_if_unset(
            "YORISHIRO_ONNX_TOKENIZER_PATH",
            config.embedding.onnx_tokenizer_path,
        );
        apply_if_unset(
            "YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH",
            config
                .embedding
                .onnx_max_sequence_length
                .map(|n| n.to_string()),
        );

        apply_if_unset("YORISHIRO_LOG_TARGET", config.logging.target);
        apply_if_unset("YORISHIRO_LOG_DIR", config.logging.dir);
        apply_if_unset("YORISHIRO_SYSLOG_SOCKET", config.logging.syslog_socket);

        apply_if_unset(
            "YORISHIRO_DB_LOAD_THRESHOLD",
            config.db_load_guard.threshold.map(|n| n.to_string()),
        );
        apply_if_unset(
            "YORISHIRO_DB_LOAD_POLL_SECS",
            config.db_load_guard.poll_secs.map(|n| n.to_string()),
        );
        apply_if_unset(
            "YORISHIRO_DB_LOAD_SUSTAIN_SECS",
            config.db_load_guard.sustain_secs.map(|n| n.to_string()),
        );

        apply_if_unset(
            "YORISHIRO_AUTH_RATE_LIMIT_MAX",
            config.auth_rate_limit.max.map(|n| n.to_string()),
        );
        apply_if_unset(
            "YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS",
            config.auth_rate_limit.window_secs.map(|n| n.to_string()),
        );
        apply_if_unset(
            "YORISHIRO_SEARCH_TOKENS_PER_MINUTE",
            config.search_tokens_per_minute.map(|n| n.to_string()),
        );
        apply_if_unset(
            "YORISHIRO_SNAPSHOT_RETENTION_DAYS",
            config.snapshot_retention_days.map(|n| n.to_string()),
        );
    }

    Ok(())
}

#[cfg(test)]
#[path = "../../tests/config/mod.rs"]
mod tests;
