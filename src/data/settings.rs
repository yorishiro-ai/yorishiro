use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub(crate) max_tenants: i32,
    pub embedding: Embedding,
    pub(crate) rate_limit: RateLimit,
    pub(crate) db_load_guard: DbLoadGuard,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Embedding {
    pub provider: EmbeddingProvider,
    pub(crate) dimensions: usize,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: String,
    pub(crate) model: Option<String>,
    pub(crate) send_dimensions_param: bool,
    pub(crate) local_model: String,
    pub(crate) local_max_sequence_length: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    None,
    Local,
    Openai,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RateLimit {
    pub(crate) auth_max_requests: u32,
    pub(crate) auth_window_seconds: u64,
    pub(crate) search_tokens_per_minute: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbLoadGuard {
    pub(crate) threshold: i64,
    pub(crate) sustain_seconds: u64,
    pub(crate) poll_seconds: u64,
}
