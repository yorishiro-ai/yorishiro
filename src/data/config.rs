//! Loads `config/<environment>.yaml` and rejects settings the application cannot run with.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use loco_rs::{
    Error, Result,
    config::{Config, QueueConfig, WorkerMode},
    environment::Environment,
};
use sea_orm::sqlx::sqlite::SqliteConnectOptions;

use crate::error::YorishiroError;

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn load(environment: &Environment) -> Result<Config> {
    let folder = crate::data::env::config_folder().map(PathBuf::from);
    let path = folder.as_deref().unwrap_or_else(|| Path::new("config"));
    let config = if matches!(environment, Environment::Production)
        && !path.join("production.yaml").try_exists()?
        && !path.join("production.local.yaml").try_exists()?
    {
        load_production_defaults(environment)?
    } else {
        match folder.as_deref() {
            Some(folder) => Config::from_folder(environment, folder)?,
            None => Config::new(environment)?,
        }
    };
    validate(&config)?;
    Ok(config)
}

fn load_production_defaults(environment: &Environment) -> Result<Config> {
    // Loco's loader takes a directory and applies its environment templating.
    let folder = tempfile::tempdir()?;
    fs::write(
        folder.path().join("production.yaml"),
        include_str!("../../config/production.yaml"),
    )?;
    Config::from_folder(environment, folder.path())
}

/// Validates application-owned settings that are stricter than Loco's generic config.
///
/// # Errors
/// Returns an error when a setting is invalid.
pub fn validate(config: &Config) -> Result<()> {
    validate_queue_policy(config)?;
    validate_http_config(config)?;
    validate_settings(config)
}

fn validate_http_config(config: &Config) -> Result<()> {
    if let Some(cors) = config
        .server
        .middlewares
        .cors
        .as_ref()
        .filter(|cors| cors.enable)
    {
        for origin in &cors.allow_origins {
            let valid = origin.parse::<axum::http::Uri>().is_ok_and(|uri| {
                matches!(uri.scheme_str(), Some("http" | "https"))
                    && uri
                        .authority()
                        .is_some_and(|authority| !authority.as_str().contains(['*', '@']))
                    && uri.path() == "/"
                    && uri.query().is_none()
                    && !origin.ends_with('/')
            });
            if !valid {
                return Err(Error::Message(
                    "server.middlewares.cors.allow_origins requires exact http(s) origins without wildcards or paths".into(),
                ));
            }
        }
        let _ = cors.cors()?;
    }
    Ok(())
}

fn validate_settings(config: &Config) -> Result<()> {
    let settings = config.settings::<crate::data::settings::Settings>()?;
    crate::workers::target::targets_for_start(&settings.worker_targets)
        .map_err(|error| Error::Message(format!("settings.worker_targets: {error}")))?;
    crate::data::time::parse_timezone(&settings.timezone).map_err(|_| {
        Error::Message(format!(
            "settings.timezone must be a valid IANA timezone, got {:?}",
            settings.timezone
        ))
    })?;
    let t = &settings.http_timeouts;
    if [
        t.embedding_request_seconds,
        t.local_embedding_seconds,
        t.model_fetch_connect_seconds,
        t.model_fetch_read_seconds,
        t.inference_request_seconds,
        t.oauth_discovery_seconds,
    ]
    .contains(&0)
    {
        return Err(Error::Message(
            "settings.http_timeouts values must be greater than zero".into(),
        ));
    }
    if settings.max_tenants < 0 {
        return Err(Error::Message(
            "settings.max_tenants must not be negative".into(),
        ));
    }
    if settings
        .mcp
        .allowed_hosts
        .as_deref()
        .is_some_and(|hosts| hosts.split(',').any(|host| host.trim() == "*"))
    {
        return Err(Error::Message(
            "settings.mcp.allowed_hosts must not contain wildcard hosts".into(),
        ));
    }
    if settings.embedding.dimensions == 0
        || settings.embedding.local_max_sequence_length == 0
        || settings.rate_limit.auth_max_requests == 0
        || settings.rate_limit.auth_window_seconds == 0
        || settings.rate_limit.search_tokens_per_minute == 0
        || settings.db_load_guard.sustain_seconds == 0
        || settings.db_load_guard.poll_seconds == 0
        || settings.embedding.provider_concurrency == 0
    {
        return Err(Error::Message(
            "embedding dimensions, provider concurrency, sequence length, rate limits, and database load guard durations must be greater than zero".into(),
        ));
    }
    validate_query_embedding(&settings.query_embedding)?;
    let dispatch = settings.dispatch;
    if dispatch.poll_interval_ms == 0
        || dispatch.enqueue_timeout_ms == 0
        || dispatch.publish_concurrency == 0
    {
        return Err(Error::Message(
            "settings.dispatch poll_interval_ms, enqueue_timeout_ms and publish_concurrency must be greater than zero"
                .into(),
        ));
    }
    if u64::from(dispatch.claim_lease_seconds) * 1000 <= dispatch.enqueue_timeout_ms {
        return Err(Error::Message(
            "settings.dispatch.claim_lease_seconds must exceed enqueue_timeout_ms".into(),
        ));
    }
    Ok(())
}

/// The longest a search may be told to wait for a worker, so a typo cannot hold a request open for hours.
const MAX_QUERY_EMBEDDING_TIMEOUT_MS: u64 = 120_000;

fn validate_query_embedding(settings: &crate::data::settings::QueryEmbedding) -> Result<()> {
    if settings.timeout_ms == 0 || settings.poll_interval_ms == 0 {
        return Err(Error::Message(
            "settings.query_embedding.timeout_ms and poll_interval_ms must be greater than zero"
                .into(),
        ));
    }
    if settings.timeout_ms > MAX_QUERY_EMBEDDING_TIMEOUT_MS {
        return Err(Error::Message(format!(
            "settings.query_embedding.timeout_ms must not exceed {MAX_QUERY_EMBEDDING_TIMEOUT_MS}"
        )));
    }
    if settings.poll_interval_ms > settings.timeout_ms {
        return Err(Error::Message(
            "settings.query_embedding.poll_interval_ms must not exceed timeout_ms".into(),
        ));
    }
    if settings.retention_seconds.saturating_mul(1000) < settings.timeout_ms {
        return Err(Error::Message(
            "settings.query_embedding.retention_seconds must cover timeout_ms".into(),
        ));
    }
    Ok(())
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn validate_queue_policy(config: &Config) -> Result<()> {
    if config.workers.mode != WorkerMode::BackgroundQueue {
        return Err(Error::Message(
            "the configured worker mode is not supported; use the built-in queue mode".into(),
        ));
    }
    let Some(queue) = config.queue.as_ref() else {
        return Err(Error::Message(
            "BackgroundQueue requires a configured queue provider".into(),
        ));
    };
    // Restrict the existing SQL providers only; future providers validate their own backend requirements.
    let sqlite_database = is_sqlite_uri(&config.database.uri);
    let mismatch = match queue {
        QueueConfig::Postgres(_) if sqlite_database => {
            Some("a PostgreSQL queue is not allowed with a SQLite database")
        }
        QueueConfig::Sqlite(_) if !sqlite_database => {
            Some("a SQLite queue is not allowed with a PostgreSQL database")
        }
        _ => None,
    };
    if let Some(message) = mismatch {
        return Err(YorishiroError::ValidationFailed {
            message: message.into(),
            details: vec![],
            hint: "Use a SQL queue matching the database, or a Redis-compatible queue".into(),
        }
        .into());
    }
    let workers = match queue {
        QueueConfig::Postgres(queue) => queue.num_workers,
        QueueConfig::Redis(queue) => queue.num_workers,
        QueueConfig::Sqlite(queue) => queue.num_workers,
        _ => {
            return Err(Error::Message(
                "queue provider is not supported by this build".into(),
            ));
        }
    };
    if workers == 0 {
        return Err(Error::Message(
            "queue.num_workers must be greater than zero for deterministic scheduling".into(),
        ));
    }
    reject_shared_sqlite_file(config, queue)?;
    Ok(())
}

/// The most connections a SQLite database pool is expected to need.
///
/// SQLite admits one writer at a time, so connections beyond a small pool only queue behind its write lock and surface as `SQLITE_BUSY` waits.
pub const SQLITE_POOL_ADVISORY_LIMIT: u32 = 16;

/// Valid settings that cannot add throughput on the configured database and queue pair.
///
/// A SQLite queue dequeues under `BEGIN IMMEDIATE`, so its workers serialize on one write lock, and a SQLite database has the same single writer.
/// PostgreSQL claims queue rows with `FOR UPDATE SKIP LOCKED` and Redis-compatible queues need no database lock, so they are the topologies where `num_workers` scales.
/// PostgreSQL with a Redis-compatible queue additionally keeps queue polling off the database.
#[must_use]
pub fn topology_advisories(config: &Config) -> Vec<String> {
    let mut advisories = Vec::new();
    if let Some(QueueConfig::Sqlite(queue)) = config.queue.as_ref()
        && queue.num_workers > 1
    {
        advisories.push(format!(
            "queue.num_workers is {} on a SQLite queue, whose dequeue serializes behind one write lock: extra workers add contention, not throughput; use 1, or a Redis-compatible queue (a PostgreSQL queue is supported only with a PostgreSQL database)",
            queue.num_workers
        ));
    }
    if is_sqlite_uri(&config.database.uri)
        && config.database.max_connections > SQLITE_POOL_ADVISORY_LIMIT
    {
        advisories.push(format!(
            "database.max_connections is {} on SQLite, which has a single writer: connections beyond {SQLITE_POOL_ADVISORY_LIMIT} only wait on its write lock",
            config.database.max_connections
        ));
    }
    let sqlite_database = is_sqlite_uri(&config.database.uri);
    if sqlite_database && matches!(config.queue.as_ref(), Some(QueueConfig::Redis(_))) {
        advisories.push(
            "SQLite with a Redis-compatible queue is supported, but keep every worker on the database's host: remote workers cannot open its SQLite file, and SQLite's named locks are no-ops".into(),
        );
    }
    advisories
}

/// Makes a Redis queue poll every named queue the workers enqueue to.
///
/// Redis workers poll only the queues the configuration names, so a job sent to any other would sit there with nothing ever dequeuing it.
/// The SQL providers ignore queue names and are left alone.
pub fn serve_worker_queues(config: &mut Config, queues: &[String]) {
    let Some(QueueConfig::Redis(redis)) = config.queue.as_mut() else {
        return;
    };
    let served = redis.queues.get_or_insert_with(Vec::new);
    for queue in queues {
        if !served.contains(queue) {
            served.push(queue.clone());
        }
    }
}

fn reject_shared_sqlite_file(config: &Config, queue: &QueueConfig) -> Result<()> {
    let QueueConfig::Sqlite(queue) = queue else {
        return Ok(());
    };
    if !is_sqlite_uri(&config.database.uri) {
        return Ok(());
    }

    let Some(database_file) = sqlite_file_identity(&config.database.uri)? else {
        return Ok(());
    };
    let Some(queue_file) = sqlite_file_identity(&queue.uri)? else {
        return Ok(());
    };
    if database_file == queue_file {
        return Err(Error::Message(format!(
            "SQLite database and queue resolve to the same file ({}); set QUEUE_URL or queue.uri to a separate SQLite file such as sqlite://yorishiro_queue.sqlite3?mode=rwc",
            database_file.display()
        )));
    }
    Ok(())
}

fn is_sqlite_uri(uri: &str) -> bool {
    uri.starts_with("sqlite:")
}

fn sqlite_file_identity(uri: &str) -> Result<Option<PathBuf>> {
    let options = SqliteConnectOptions::from_str(uri)
        .map_err(|err| Error::Message(format!("invalid SQLite URI {uri:?}: {err}")))?;
    let filename = options.get_filename();
    if filename.as_os_str().is_empty()
        || filename
            .to_string_lossy()
            .starts_with("file:sqlx-in-memory-")
    {
        return Ok(None);
    }

    let path = if filename.is_absolute() {
        filename.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| Error::Message(format!("failed to resolve SQLite URI path: {err}")))?
            .join(filename)
    };
    Ok(Some(normalize_sqlite_path(&path)))
}

fn normalize_sqlite_path(path: &Path) -> PathBuf {
    let mut unresolved = path.to_path_buf();
    let mut suffix = Vec::new();
    while !unresolved.exists() {
        if let Some(name) = unresolved.file_name() {
            suffix.push(name.to_owned());
        }
        if !unresolved.pop() {
            break;
        }
    }
    let base = fs::canonicalize(&unresolved).unwrap_or(unresolved);
    let mut normalized = lexical_normalize(&base);
    for component in suffix.iter().rev() {
        normalized.push(component);
    }
    lexical_normalize(&normalized)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
