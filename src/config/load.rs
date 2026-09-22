use std::{
    env, fs,
    path::{Path, PathBuf},
};

use loco_rs::{Error, Result, config::Config, environment::Environment};

use super::{CANONICAL_CONFIG_FILE, CONFIG_PATH_ENV, validate};

pub async fn load(environment: &Environment) -> Result<Config> {
    let base_dir = env::current_dir()
        .map_err(|err| Error::Message(format!("failed to resolve the current directory: {err}")))?;
    load_from(&base_dir, environment).await
}

pub(super) async fn load_from(base_dir: &Path, environment: &Environment) -> Result<Config> {
    let environment = test_environment(environment);
    if let Some(path) = env::var_os(CONFIG_PATH_ENV) {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            base_dir.join(path)
        };
        return validate::load_canonical(&path);
    }

    let canonical = base_dir.join(CANONICAL_CONFIG_FILE);
    match fs::symlink_metadata(&canonical) {
        Ok(_) => validate::load_canonical(&canonical),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let result = environment.load();
            if result.is_ok() {
                eprintln!(
                    "warning: using legacy config/{environment}.yaml; migrate to {CANONICAL_CONFIG_FILE} before its removal target of 0.61.0"
                );
            }
            result
        }
        Err(err) => Err(Error::Message(format!(
            "failed to inspect {}: {err}",
            canonical.display()
        ))),
    }
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
