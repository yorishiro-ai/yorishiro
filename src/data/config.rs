//! Loads `config/<environment>.yaml` and rejects settings the application cannot run with.

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use loco_rs::{
    Error, Result,
    config::{Config, QueueConfig, WorkerMode},
    environment::Environment,
};
use sqlx::sqlite::SqliteConnectOptions;

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn load(environment: &Environment) -> Result<Config> {
    let environment = test_environment(environment);
    let config = load_environment(&environment)?;
    validate(&config)?;
    Ok(config)
}

const EMBEDDED_EXAMPLE: &[u8] = include_bytes!("../../config/example.yaml");

fn load_environment(environment: &Environment) -> Result<Config> {
    let folder = env::var("LOCO_CONFIG_FOLDER")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config"));
    let base = folder.join(format!("{environment}.yaml"));
    let local = folder.join(format!("{environment}.local.yaml"));
    let base_present = config_file_state(&base)?;
    let local_present = config_file_state(&local)?;

    if local_present {
        if !base_present {
            return Err(Error::Message(format!(
                "configuration override exists without base configuration: {}",
                local.display()
            )));
        }
        return environment.load_from_folder(&folder);
    }

    if base_present {
        return environment.load_from_folder(&folder);
    }

    tracing::info!(environment = %environment, "using embedded example configuration defaults");
    let temp = tempfile_path();
    fs::create_dir(&temp).map_err(|error| {
        Error::Message(format!(
            "could not create embedded configuration directory {}: {error}",
            temp.display()
        ))
    })?;
    let embedded = temp.join(format!("{environment}.yaml"));
    fs::write(&embedded, EMBEDDED_EXAMPLE).map_err(|error| {
        Error::Message(format!(
            "could not materialize embedded configuration: {error}"
        ))
    })?;
    let result = environment.load_from_folder(&temp);
    let _ = fs::remove_file(&embedded);
    let _ = fs::remove_dir(&temp);
    result
}

fn tempfile_path() -> PathBuf {
    env::temp_dir().join(format!(
        "yorishiro-config-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ))
}

fn config_file_state(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(Error::Message(format!(
            "configuration path is not a file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::Message(format!(
            "cannot access configuration file {}: {error}",
            path.display()
        ))),
    }
}

fn test_environment(environment: &Environment) -> Environment {
    match environment {
        Environment::Test => {
            if env::var("QUEUE_URL")
                .is_ok_and(|url| url.starts_with("redis://") || url.starts_with("rediss://"))
            {
                return Environment::Any("test_redis".into());
            }
            let url = env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://loco:loco@localhost:5432/yorishiro_test".into());
            if url.starts_with("sqlite://") || url.starts_with("sqlite::") {
                Environment::Any("test_sqlite".into())
            } else {
                Environment::Any("test_postgres".into())
            }
        }
        other => other.clone(),
    }
}

fn validate(config: &Config) -> Result<()> {
    let settings = config.settings::<crate::data::settings::Settings>()?;
    if settings.max_tenants < 0 {
        return Err(Error::Message(
            "settings.max_tenants must not be negative".into(),
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
    validate_queue_policy(config)
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
    uri.starts_with("sqlite://") || uri.starts_with("sqlite::")
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
