use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub(crate) max_tenants: i32,
    pub embedding: Embedding,
    pub(crate) rate_limit: RateLimit,
    pub(crate) db_load_guard: DbLoadGuard,
    #[serde(default)]
    pub query_embedding: QueryEmbedding,
}

/// How the API and MCP server waits for a worker to embed a search query.
///
/// The server holds no embedding model: it enqueues a request and polls for the result, so these three values bound that wait.
/// The section is optional so a configuration file written before it existed keeps loading with the defaults.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct QueryEmbedding {
    /// The longest a search waits for its vector, in milliseconds.
    pub timeout_ms: u64,
    /// The pause between two reads of the result row, in milliseconds.
    pub poll_interval_ms: u64,
    /// How long an unconsumed request or result row is kept before it is purged, in seconds.
    pub retention_seconds: u64,
}

impl Default for QueryEmbedding {
    fn default() -> Self {
        Self {
            timeout_ms: 15_000,
            poll_interval_ms: 100,
            retention_seconds: 300,
        }
    }
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
