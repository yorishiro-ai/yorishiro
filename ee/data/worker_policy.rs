//! Enterprise-only worker admission limits, applied only when plan policy is enabled.

use crate::edition::ee::data::env::Env;

pub(crate) struct WorkerClassLimits {
    pub(crate) shared: i32,
    pub(crate) tenant_private: i32,
    pub(crate) official: Option<i32>,
}

impl WorkerClassLimits {
    pub(crate) fn from_env() -> Result<Self, loco_rs::Error> {
        fn limit(key: Env) -> Result<Option<i32>, loco_rs::Error> {
            match key.non_empty() {
                None => Ok(None),
                Some(value) => value
                    .parse::<i32>()
                    .ok()
                    .filter(|value| *value > 0)
                    .map(Some)
                    .ok_or_else(|| {
                        loco_rs::Error::Message(format!(
                            "{} must be a positive integer",
                            key.name()
                        ))
                    }),
            }
        }

        Ok(Self {
            shared: limit(Env::WorkerClassSharedLimit)?.unwrap_or(1),
            tenant_private: limit(Env::WorkerClassTenantPrivateLimit)?.unwrap_or(1),
            official: limit(Env::WorkerOfficialLimit)?,
        })
    }
}
