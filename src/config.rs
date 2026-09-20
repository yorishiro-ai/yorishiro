use std::{
    env, fs,
    path::{Path, PathBuf},
};

use loco_rs::{Error, Result, config::Config, environment::Environment};
use serde_yaml::{Mapping, Number, Value};

pub const CANONICAL_CONFIG_FILE: &str = "yorishiro.yaml";
const CONFIG_PATH_ENV: &str = "YORISHIRO_CONFIG_PATH";

pub const CONFIG_SKELETON: &str = r#"# Yorishiro configuration.
#
# This is plain YAML. Yorishiro does not evaluate Tera templates or get_env expressions here.
# Environment variables listed in the comments below explicitly override these values.

# Logging. LOG_LEVEL overrides logger.level. RUST_LOG has final control over filtering.
logger:
  enable: true
  pretty_backtrace: false
  level: info
  format: compact

# HTTP server. PORT, BINDING, and HOST override the corresponding values.
server:
  port: 5150
  binding: 0.0.0.0
  host: http://localhost
  middlewares:
    request_id:
      enable: true
    logger:
      enable: true

# Database. DATABASE_URL, DB_LOGGING, DB_CONNECT_TIMEOUT, DB_IDLE_TIMEOUT, DB_MIN_CONNECTIONS, DB_MAX_CONNECTIONS, and DB_AUTO_MIGRATE override these values.
# SQLite is for a single tenant. Use PostgreSQL for multi-tenant deployments.
database:
  # The CLI skeleton uses a writable path relative to the working directory.
  # Package and Docker installations replace this with /var/lib/yorishiro.
  uri: sqlite://yorishiro.sqlite3?mode=rwc
  enable_logging: false
  connect_timeout: 500
  idle_timeout: 500
  min_connections: 1
  max_connections: 100
  auto_migrate: true
  dangerously_truncate: false
  dangerously_recreate: false

# Background jobs. QUEUE_URL, YORISHIRO_QUEUE_WORKERS, and YORISHIRO_QUEUE_REAPER_AGE_MINUTES override fields in this block.
# YORISHIRO_QUEUE_KIND can switch between Sqlite, Postgres, and Redis.
workers:
  mode: BackgroundQueue
queue:
  kind: Sqlite
  uri: sqlite://yorishiro.sqlite3?mode=rwc
  dangerously_flush: false
  num_workers: 2
  reaper:
    age_minutes: 30
    interval_seconds: 60

# Add scheduled tasks here when this process runs `yorishiro scheduler`.
# scheduler:
#   output: stdout
#   jobs: {}

# Configure mail only when the deployment sends email. MAILER_HOST, MAILER_PORT, MAILER_USER, and MAILER_PASSWORD override a configured SMTP block.
# mailer:
#   smtp:
#     enable: true
#     host: smtp.example.com
#     port: 587
#     secure: true
#     auth:
#       user: example
#       password: change-me

# Application and enterprise settings are intentionally environment-only.
# Worker startup tags use YORISHIRO_WORKER_TAGS (comma-separated):
# worker-class:tenant-private, worker-class:official, worker-class:shared, or infer-fill.
# Scheduler jobs may be declared in scheduler.jobs and run with `yorishiro scheduler`.
# Embedding variables include YORISHIRO_EMBEDDING_PROVIDER, YORISHIRO_EMBEDDING_MODEL,
# YORISHIRO_EMBEDDING_BASE_URL, and YORISHIRO_EMBEDDING_API_KEY.
# Limits and operations include YORISHIRO_MAX_TENANTS, YORISHIRO_LOAD_GUARD_*,
# and YORISHIRO_RATE_LIMIT_*.
# Enterprise variables (licence, OAuth/OIDC, Stripe, marketplace, and worker classes)
# are read directly by `ee/`; they are deliberately not duplicated as YAML fields.
"#;

pub async fn load(environment: &Environment) -> Result<Config> {
    let base_dir = env::current_dir()
        .map_err(|err| Error::Message(format!("failed to resolve the current directory: {err}")))?;
    load_from(&base_dir, environment).await
}

async fn load_from(base_dir: &Path, environment: &Environment) -> Result<Config> {
    let environment = test_environment(environment);
    if let Some(path) = env::var_os(CONFIG_PATH_ENV) {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            base_dir.join(path)
        };
        return load_canonical(&path);
    }

    let canonical = base_dir.join(CANONICAL_CONFIG_FILE);
    match fs::symlink_metadata(&canonical) {
        Ok(_) => load_canonical(&canonical),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => environment.load(),
        Err(err) => Err(Error::Message(format!(
            "failed to inspect {}: {err}",
            canonical.display()
        ))),
    }
}

pub fn init(force: bool) -> Result<()> {
    let base_dir = env::current_dir()
        .map_err(|err| Error::Message(format!("failed to resolve the current directory: {err}")))?;
    init_at(&base_dir.join(CANONICAL_CONFIG_FILE), force)
}

fn init_at(path: &Path, force: bool) -> Result<()> {
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(Error::Message(format!(
                "failed to inspect {}: {err}",
                path.display()
            )));
        }
    };

    if let Some(metadata) = existing {
        if !force {
            return Err(Error::Message(format!(
                "refusing to overwrite {}; rerun with --force to replace it",
                path.display()
            )));
        }
        eprintln!("overwriting {}", path.display());
        if metadata.file_type().is_symlink() {
            fs::remove_file(path).map_err(|err| {
                Error::Message(format!("failed to remove {}: {err}", path.display()))
            })?;
            write_new_skeleton(path)?;
        } else {
            fs::write(path, CONFIG_SKELETON).map_err(|err| {
                Error::Message(format!("failed to write {}: {err}", path.display()))
            })?;
        }
    } else {
        write_new_skeleton(path)?;
    }
    println!("created {}", path.display());
    Ok(())
}

fn write_new_skeleton(path: &Path) -> Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| Error::Message(format!("failed to create {}: {err}", path.display())))?;
    file.write_all(CONFIG_SKELETON.as_bytes())
        .map_err(|err| Error::Message(format!("failed to write {}: {err}", path.display())))
}

fn test_environment(environment: &Environment) -> Environment {
    match environment {
        Environment::Test => {
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

fn load_canonical(path: &Path) -> Result<Config> {
    let source = fs::read_to_string(path)
        .map_err(|err| Error::Message(format!("failed to read {}: {err}", path.display())))?;
    if source.contains("<%") || source.contains("{{") || source.contains("get_env(") {
        return Err(Error::Message(format!(
            "{} must be plain YAML; Tera template syntax is not supported",
            path.display()
        )));
    }
    let mut value: Value = serde_yaml::from_str(&source)
        .map_err(|err| Error::YAMLFile(err, path.display().to_string()))?;
    apply_environment_overrides(&mut value)?;
    serde_yaml::from_value(value).map_err(|err| Error::YAMLFile(err, path.display().to_string()))
}

fn apply_environment_overrides(value: &mut Value) -> Result<()> {
    set_string(value, &["logger", "level"], "LOG_LEVEL");
    set_i64(value, &["server", "port"], "PORT")?;
    set_string(value, &["server", "binding"], "BINDING");
    set_string(value, &["server", "host"], "HOST");
    let database_url = env::var("DATABASE_URL").ok();
    if let Some(uri) = &database_url {
        set_value(value, &["database", "uri"], Value::String(uri.clone()));
    }
    set_bool(value, &["database", "enable_logging"], "DB_LOGGING")?;
    set_u64(
        value,
        &["database", "connect_timeout"],
        "DB_CONNECT_TIMEOUT",
    )?;
    set_u64(value, &["database", "idle_timeout"], "DB_IDLE_TIMEOUT")?;
    set_u64(
        value,
        &["database", "min_connections"],
        "DB_MIN_CONNECTIONS",
    )?;
    set_u64(
        value,
        &["database", "max_connections"],
        "DB_MAX_CONNECTIONS",
    )?;
    set_bool(value, &["database", "auto_migrate"], "DB_AUTO_MIGRATE")?;
    set_bool(
        value,
        &["database", "dangerously_truncate"],
        "DANGEROUSLY_TRUNCATE",
    )?;
    set_bool(
        value,
        &["database", "dangerously_recreate"],
        "DANGEROUSLY_RECREATE",
    )?;
    set_u64(value, &["queue", "num_workers"], "YORISHIRO_QUEUE_WORKERS")?;
    set_i64(
        value,
        &["queue", "reaper", "age_minutes"],
        "YORISHIRO_QUEUE_REAPER_AGE_MINUTES",
    )?;
    apply_queue_overrides(value, database_url.as_deref())?;
    apply_mailer_overrides(value)?;
    Ok(())
}

fn apply_queue_overrides(value: &mut Value, database_url: Option<&str>) -> Result<()> {
    let explicit_kind = env::var("YORISHIRO_QUEUE_KIND").ok();
    let configured_kind = queue_kind(value).map(str::to_owned);
    let kind = match explicit_kind.as_deref() {
        Some("Sqlite") => "Sqlite",
        Some("Postgres") => "Postgres",
        Some("Redis") => "Redis",
        Some(_) => {
            return Err(Error::Message(
                "YORISHIRO_QUEUE_KIND must be Sqlite, Postgres, or Redis".into(),
            ));
        }
        None => database_url
            .and_then(queue_kind_for_uri)
            .unwrap_or(configured_kind.as_deref().unwrap_or("Sqlite")),
    };

    let queue_url = env::var("QUEUE_URL").ok();
    if kind == "Redis" && queue_url.is_none() {
        return Err(Error::Message(
            "QUEUE_URL is required when the queue kind is Redis".into(),
        ));
    }

    let configured_uri = queue_uri(value);
    let kind = kind.to_owned();
    if let Some(uri) = &queue_url {
        if queue_kind_for_uri(uri) != Some(kind.as_str()) {
            return Err(Error::Message(format!(
                "QUEUE_URL is not compatible with queue kind {kind}"
            )));
        }
    }
    let uri = queue_url
        .or_else(|| {
            database_url
                .filter(|uri| queue_kind_for_uri(uri) == Some(kind.as_str()))
                .map(str::to_owned)
        })
        .or_else(|| {
            configured_uri
                .filter(|uri| queue_kind_for_uri(uri) == Some(kind.as_str()))
                .map(str::to_owned)
        });
    if uri.is_none() {
        return Err(Error::Message(format!(
            "no queue URI is compatible with queue kind {kind}; set QUEUE_URL"
        )));
    }

    set_value(value, &["queue", "kind"], Value::String(kind));
    if let Some(uri) = uri {
        set_value(value, &["queue", "uri"], Value::String(uri));
    }
    Ok(())
}

fn queue_kind(value: &Value) -> Option<&str> {
    value
        .get("queue")
        .and_then(Value::as_mapping)
        .and_then(|map| map.get(Value::String("kind".into())))
        .and_then(Value::as_str)
}

fn queue_uri(value: &Value) -> Option<&str> {
    value
        .get("queue")
        .and_then(Value::as_mapping)
        .and_then(|map| map.get(Value::String("uri".into())))
        .and_then(Value::as_str)
}

fn queue_kind_for_uri(uri: &str) -> Option<&'static str> {
    if uri.starts_with("postgres://") || uri.starts_with("postgresql://") {
        Some("Postgres")
    } else if uri.starts_with("sqlite://") || uri.starts_with("sqlite::") {
        Some("Sqlite")
    } else if uri.starts_with("redis://") || uri.starts_with("rediss://") {
        Some("Redis")
    } else {
        None
    }
}

fn apply_mailer_overrides(value: &mut Value) -> Result<()> {
    let host = env::var("MAILER_HOST").ok();
    let port = env::var("MAILER_PORT").ok();
    let user = env::var("MAILER_USER").ok();
    let password = env::var("MAILER_PASSWORD").ok();
    if host.is_none() && port.is_none() && user.is_none() && password.is_none() {
        return Ok(());
    }
    if let Some(host) = host {
        set_value(value, &["mailer", "smtp", "enable"], Value::Bool(true));
        set_value(value, &["mailer", "smtp", "host"], Value::String(host));
        set_value(value, &["mailer", "smtp", "secure"], Value::Bool(true));
        if port.is_none() {
            set_value(
                value,
                &["mailer", "smtp", "port"],
                Value::Number(Number::from(587)),
            );
        }
    }
    if let Some(port) = port {
        let parsed = port
            .parse::<u16>()
            .map_err(|_| invalid_override("MAILER_PORT", "a valid SMTP port"))?;
        set_value(
            value,
            &["mailer", "smtp", "port"],
            Value::Number(Number::from(parsed)),
        );
    }
    if let Some(user) = user {
        set_value(
            value,
            &["mailer", "smtp", "auth", "user"],
            Value::String(user),
        );
    }
    if let Some(password) = password {
        set_value(
            value,
            &["mailer", "smtp", "auth", "password"],
            Value::String(password),
        );
    }
    Ok(())
}

fn set_string(value: &mut Value, path: &[&str], variable: &str) {
    if let Ok(value_from_env) = env::var(variable) {
        set_value(value, path, Value::String(value_from_env));
    }
}

fn set_bool(value: &mut Value, path: &[&str], variable: &str) -> Result<()> {
    if let Ok(raw) = env::var(variable) {
        let parsed = raw
            .parse::<bool>()
            .map_err(|_| invalid_override(variable, "true or false"))?;
        set_value(value, path, Value::Bool(parsed));
    }
    Ok(())
}

fn set_u64(value: &mut Value, path: &[&str], variable: &str) -> Result<()> {
    if let Ok(raw) = env::var(variable) {
        let parsed = raw
            .parse::<u64>()
            .map_err(|_| invalid_override(variable, "a non-negative integer"))?;
        set_value(value, path, Value::Number(Number::from(parsed)));
    }
    Ok(())
}

fn set_i64(value: &mut Value, path: &[&str], variable: &str) -> Result<()> {
    if let Ok(raw) = env::var(variable) {
        let parsed = raw
            .parse::<i64>()
            .map_err(|_| invalid_override(variable, "an integer"))?;
        set_value(value, path, Value::Number(Number::from(parsed)));
    }
    Ok(())
}

fn invalid_override(variable: &str, expected: &str) -> Error {
    Error::Message(format!("{variable} must be {expected}"))
}

fn set_value(value: &mut Value, path: &[&str], replacement: Value) {
    let mut current = value;
    for key in &path[..path.len() - 1] {
        if !matches!(current, Value::Mapping(_)) {
            *current = Value::Mapping(Mapping::new());
        }
        let Value::Mapping(mapping) = current else {
            unreachable!()
        };
        current = mapping
            .entry(Value::String((*key).into()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
    }
    if !matches!(current, Value::Mapping(_)) {
        *current = Value::Mapping(Mapping::new());
    }
    let Value::Mapping(mapping) = current else {
        unreachable!()
    };
    mapping.insert(Value::String(path[path.len() - 1].into()), replacement);
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

    fn minimal_config(uri: &str) -> String {
        format!(
            "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: {uri}\nworkers:\n  mode: ForegroundBlocking\n"
        )
    }

    struct EnvGuard {
        values: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn capture(variables: &[&'static str]) -> Self {
            Self {
                values: variables
                    .iter()
                    .map(|variable| (*variable, env::var_os(variable)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                for (variable, original) in &self.values {
                    match original {
                        Some(value) => env::set_var(variable, value),
                        None => env::remove_var(variable),
                    }
                }
            }
        }
    }

    #[tokio::test]
    #[serial]
    async fn explicit_path_loads_and_environment_overrides_it() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("explicit.yaml");
        fs::write(
            &path,
            minimal_config("sqlite:///from-file.sqlite3?mode=rwc"),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[CONFIG_PATH_ENV, "DATABASE_URL"]);
        unsafe {
            env::set_var(CONFIG_PATH_ENV, &path);
            env::set_var("DATABASE_URL", "sqlite:///from-env.sqlite3?mode=rwc");
        }
        let config = load(&Environment::Development).await.unwrap();
        assert_eq!(config.database.uri, "sqlite:///from-env.sqlite3?mode=rwc");
    }

    #[tokio::test]
    #[serial]
    async fn explicit_missing_path_never_falls_back() {
        let directory = tempdir().unwrap();
        let _guard = EnvGuard::capture(&[CONFIG_PATH_ENV]);
        unsafe { env::set_var(CONFIG_PATH_ENV, directory.path().join("missing.yaml")) };
        let error = load(&Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to read"));
    }

    #[tokio::test]
    #[serial]
    async fn explicit_unreadable_path_never_falls_back() {
        let directory = tempdir().unwrap();
        let _guard = EnvGuard::capture(&[CONFIG_PATH_ENV]);
        unsafe { env::set_var(CONFIG_PATH_ENV, directory.path()) };
        let error = load(&Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to read"));
    }

    #[tokio::test]
    #[serial]
    async fn explicit_invalid_path_never_falls_back() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("explicit.yaml");
        fs::write(&path, "logger: [not valid for Config\n").unwrap();
        let _guard = EnvGuard::capture(&[CONFIG_PATH_ENV]);
        unsafe { env::set_var(CONFIG_PATH_ENV, path) };
        let error = load(&Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("explicit.yaml"));
    }

    #[tokio::test]
    #[serial]
    async fn canonical_file_in_current_directory_wins() {
        let directory = tempdir().unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            minimal_config("sqlite:///canonical.sqlite3?mode=rwc"),
        )
        .unwrap();
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::remove_var("DATABASE_URL");
            env::remove_var("QUEUE_URL");
            env::remove_var("YORISHIRO_QUEUE_KIND");
        }
        let config = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap();
        assert_eq!(config.database.uri, "sqlite:///canonical.sqlite3?mode=rwc");
    }

    #[tokio::test]
    #[serial]
    async fn legacy_environment_file_is_the_final_fallback() {
        let _guard = EnvGuard::capture(&[CONFIG_PATH_ENV, "DATABASE_URL"]);
        unsafe { env::remove_var(CONFIG_PATH_ENV) };
        unsafe { env::set_var("DATABASE_URL", "postgres://test:test@localhost:5432/test") };
        let config = load(&Environment::Any("test_postgres".into()))
            .await
            .unwrap();
        assert_eq!(
            config.database.uri,
            "postgres://test:test@localhost:5432/test"
        );
    }

    #[tokio::test]
    #[serial]
    async fn canonical_file_rejects_tera_and_get_env_syntax() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            "server:\n  host: '{{ get_env(name=\"HOST\") }}'\n",
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe { env::remove_var(CONFIG_PATH_ENV) };
        let error = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("plain YAML"));
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial]
    async fn dangling_canonical_symlink_does_not_fall_back_to_legacy_config() {
        let directory = tempdir().unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("missing.yaml"),
            directory.path().join(CANONICAL_CONFIG_FILE),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::remove_var("DATABASE_URL");
            env::remove_var("QUEUE_URL");
            env::remove_var("YORISHIRO_QUEUE_KIND");
        }
        let error = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to read"));
    }

    #[tokio::test]
    #[serial]
    async fn postgres_database_derives_postgres_queue_and_uri() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            minimal_config("sqlite://file.sqlite3?mode=rwc"),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::set_var("DATABASE_URL", "postgres://db/app");
            env::remove_var("QUEUE_URL");
            env::remove_var("YORISHIRO_QUEUE_KIND");
        }
        let config = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap();
        assert!(matches!(
            config.queue,
            Some(loco_rs::config::QueueConfig::Postgres(queue))
                if queue.uri == "postgres://db/app"
        ));
    }

    #[tokio::test]
    #[serial]
    async fn sqlite_database_derives_sqlite_queue_and_uri() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            minimal_config("postgres://file/app"),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::set_var("DATABASE_URL", "sqlite://derived.sqlite3?mode=rwc");
            env::remove_var("QUEUE_URL");
            env::remove_var("YORISHIRO_QUEUE_KIND");
        }
        let config = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap();
        assert!(matches!(
            config.queue,
            Some(loco_rs::config::QueueConfig::Sqlite(queue))
                if queue.uri == "sqlite://derived.sqlite3?mode=rwc"
        ));
    }

    #[tokio::test]
    #[serial]
    async fn explicit_queue_kind_and_queue_url_take_precedence() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            minimal_config("sqlite://file.sqlite3?mode=rwc"),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::set_var("DATABASE_URL", "postgres://db/app");
            env::set_var("QUEUE_URL", "sqlite://queue.sqlite3?mode=rwc");
            env::set_var("YORISHIRO_QUEUE_KIND", "Sqlite");
        }
        let config = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap();
        assert!(matches!(
            config.queue,
            Some(loco_rs::config::QueueConfig::Sqlite(queue))
                if queue.uri == "sqlite://queue.sqlite3?mode=rwc"
        ));
    }

    #[tokio::test]
    #[serial]
    async fn explicit_postgres_kind_rejects_sqlite_database_without_queue_url() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            minimal_config("sqlite://file.sqlite3?mode=rwc"),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::set_var("DATABASE_URL", "sqlite://db.sqlite3?mode=rwc");
            env::remove_var("QUEUE_URL");
            env::set_var("YORISHIRO_QUEUE_KIND", "Postgres");
        }
        let error = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("no queue URI is compatible"));
    }

    #[tokio::test]
    #[serial]
    async fn explicit_postgres_kind_rejects_sqlite_queue_url() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            minimal_config("postgres://file/app"),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::set_var("DATABASE_URL", "postgres://db/app");
            env::set_var("QUEUE_URL", "sqlite://queue.sqlite3?mode=rwc");
            env::set_var("YORISHIRO_QUEUE_KIND", "Postgres");
        }
        let error = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("QUEUE_URL is not compatible"));
    }

    #[tokio::test]
    #[serial]
    async fn redis_without_queue_url_is_rejected() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CANONICAL_CONFIG_FILE),
            minimal_config("sqlite://file.sqlite3?mode=rwc"),
        )
        .unwrap();
        let _guard = EnvGuard::capture(&[
            CONFIG_PATH_ENV,
            "DATABASE_URL",
            "QUEUE_URL",
            "YORISHIRO_QUEUE_KIND",
        ]);
        unsafe {
            env::remove_var(CONFIG_PATH_ENV);
            env::remove_var("DATABASE_URL");
            env::remove_var("QUEUE_URL");
            env::set_var("YORISHIRO_QUEUE_KIND", "Redis");
        }
        let error = load_from(directory.path(), &Environment::Development)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("QUEUE_URL is required"));
    }

    #[test]
    #[serial]
    fn init_refuses_existing_file_and_force_replaces_it() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(CANONICAL_CONFIG_FILE);
        init_at(&path, false).unwrap();
        assert!(init_at(&path, false).is_err());
        fs::write(&path, "not the skeleton\n").unwrap();
        init_at(&path, true).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), CONFIG_SKELETON);
    }

    #[test]
    #[serial]
    fn init_force_creates_an_absent_target() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(CANONICAL_CONFIG_FILE);
        init_at(&path, true).unwrap();
        assert!(path.is_file());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn init_without_force_refuses_a_dangling_symlink() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(CANONICAL_CONFIG_FILE);
        let target = directory.path().join("missing.yaml");
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let error = init_at(&path, false).unwrap_err().to_string();

        assert!(error.contains("refusing to overwrite"));
        assert!(path.is_symlink());
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn init_force_replaces_a_dangling_symlink_without_following_it() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(CANONICAL_CONFIG_FILE);
        let target = directory.path().join("missing.yaml");
        std::os::unix::fs::symlink(&target, &path).unwrap();

        init_at(&path, true).unwrap();

        assert!(!path.is_symlink());
        assert_eq!(fs::read_to_string(&path).unwrap(), CONFIG_SKELETON);
        assert!(!target.exists());
    }
}
