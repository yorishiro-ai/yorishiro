use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) max_tenants: i32,
    pub(crate) rate_limit: RateLimit,
    pub(crate) db_load_guard: DbLoadGuard,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct RateLimit {
    pub(crate) auth_max_requests: u32,
    pub(crate) auth_window_seconds: u64,
    pub(crate) search_tokens_per_minute: u32,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            auth_max_requests: 10,
            auth_window_seconds: 60,
            search_tokens_per_minute: 100_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct DbLoadGuard {
    pub(crate) threshold: i64,
    pub(crate) sustain_seconds: u64,
    pub(crate) poll_seconds: u64,
}

impl Default for DbLoadGuard {
    fn default() -> Self {
        Self {
            threshold: 0,
            sustain_seconds: 30,
            poll_seconds: 5,
        }
    }
}
