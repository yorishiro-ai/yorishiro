use std::env;

use loco_rs::{Error, Result};
use serde_yaml::{Mapping, Number, Value};

pub(super) fn apply_environment_overrides(value: &mut Value) -> Result<()> {
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
