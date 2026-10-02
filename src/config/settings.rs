use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) max_tenants: i32,
    pub(crate) embedding: Embedding,
    pub(crate) rate_limit: RateLimit,
    pub(crate) db_load_guard: DbLoadGuard,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct Embedding {
    pub(crate) provider: String,
    pub(crate) dimensions: usize,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: String,
    pub(crate) model: Option<String>,
    pub(crate) send_dimensions_param: bool,
    pub(crate) local_model: String,
    pub(crate) local_max_sequence_length: usize,
}

impl Default for Embedding {
    fn default() -> Self {
        Self {
            provider: "local".into(),
            dimensions: crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS,
            base_url: None,
            api_key: String::new(),
            model: None,
            send_dimensions_param: false,
            local_model: crate::services::embedding::default_local_model().into(),
            local_max_sequence_length: 512,
        }
    }
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
