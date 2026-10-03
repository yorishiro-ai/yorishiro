use std::env;

use loco_rs::{Result, config::Config, environment::Environment};

use super::validate;

pub async fn load(environment: &Environment) -> Result<Config> {
    let environment = test_environment(environment);
    let config = environment.load()?;
    validate::validate(&config)?;
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
