use std::fs;
use std::path::Path;

use loco_rs::{
    Error, Result,
    config::{Config, QueueConfig, WorkerMode},
};
use serde_yaml::Value;

use super::overrides;

pub(super) fn load_canonical(path: &Path) -> Result<Config> {
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
    overrides::apply_environment_overrides(&mut value)?;
    let config: Config = serde_yaml::from_value(value)
        .map_err(|err| Error::YAMLFile(err, path.display().to_string()))?;
    validate_queue_policy(&config)?;
    Ok(config)
}

fn validate_queue_policy(config: &Config) -> Result<()> {
    if config.workers.mode != WorkerMode::BackgroundQueue {
        return Ok(());
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
    Ok(())
}
