use std::fs;
use std::path::Path;

use loco_rs::{Error, Result, config::Config};
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
    serde_yaml::from_value(value).map_err(|err| Error::YAMLFile(err, path.display().to_string()))
}
