//! Enterprise-only worker admission limits, applied only when plan policy is enabled.

use loco_rs::config::Config;
use serde::Deserialize;

pub(crate) struct WorkerClassLimits {
    pub(crate) shared: i32,
    pub(crate) tenant_private: i32,
    pub(crate) official: Option<i32>,
}

impl WorkerClassLimits {
    pub(crate) fn from_config(config: &Config) -> Result<Self, loco_rs::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Limit {
            Integer(i32),
            Text(String),
        }
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Limits {
            shared: Option<Limit>,
            tenant_private: Option<Limit>,
            official: Option<Limit>,
        }
        let limits: Limits = config
            .initializers
            .as_ref()
            .and_then(|values| values.get("worker_admission"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        fn limit(key: &str, value: Option<Limit>) -> Result<Option<i32>, loco_rs::Error> {
            match value {
                None => Ok(None),
                Some(Limit::Text(value)) if value.is_empty() => Ok(None),
                Some(value) => match value {
                    Limit::Integer(value) => Some(value),
                    Limit::Text(value) => value.parse::<i32>().ok(),
                }
                .filter(|value| *value > 0)
                .map(Some)
                .ok_or_else(|| {
                    loco_rs::Error::Message(format!("{} must be a positive integer", key))
                }),
            }
        }

        Ok(Self {
            shared: limit("YORISHIRO_WORKER_CLASS_SHARED_LIMIT / initializers.worker_admission.shared", limits.shared)?.unwrap_or(1),
            tenant_private: limit("YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT / initializers.worker_admission.tenant_private", limits.tenant_private)?.unwrap_or(1),
            official: limit("YORISHIRO_WORKER_OFFICIAL_LIMIT / initializers.worker_admission.official", limits.official)?,
        })
    }
}
