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

pub async fn load(environment: &Environment) -> Result<Config> {
    let environment = test_environment(environment);
    let config = environment.load()?;
    validate(&config)?;
    Ok(config)
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
    {
        return Err(Error::Message(
            "embedding dimensions and sequence length, rate limits, and database load guard durations must be greater than zero".into(),
        ));
    }
    validate_queue_policy(config)
}

pub(crate) fn validate_queue_policy(config: &Config) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::validate_queue_policy;

    fn config(database: &str, queue: &str) -> loco_rs::config::Config {
        serde_yaml::from_str(&format!(
            "logger: {{ enable: true, pretty_backtrace: false, level: info, format: compact }}\nserver: {{ port: 5150, binding: localhost, host: http://localhost }}\ndatabase: {{ uri: '{database}', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: 2, auto_migrate: false }}\nqueue: {{ kind: Sqlite, uri: '{queue}', num_workers: 1 }}\nworkers: {{ mode: BackgroundQueue }}\n"
        ))
        .unwrap()
    }

    #[test]
    fn equivalent_sqlite_paths_are_rejected_but_memory_is_not() {
        let shared = config(
            "sqlite://app.sqlite3?mode=rwc",
            "sqlite://./app.sqlite3?mode=ro",
        );
        assert!(validate_queue_policy(&shared).is_err());

        let memory = config("sqlite::memory:", "sqlite://?mode=memory&cache=shared");
        assert!(validate_queue_policy(&memory).is_ok());
    }
}
