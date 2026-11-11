use serde::{Deserialize, Serialize};

/// Selects the handlers and resources a queue consumer owns.
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerRole {
    #[default]
    Embedding,
    Jobs,
}

/// Where application logs go.
#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogOutput {
    /// Standard output in the Loco `logger.format`.
    #[default]
    Stdout,
    /// The systemd journal, with levels and fields mapped natively.
    Journald,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Internal process role selected from the CLI worker target.
    #[serde(default)]
    pub(crate) consumer_role: ConsumerRole,
    /// Deployment-declared worker target tags, not a live worker inventory.
    #[serde(default = "default_worker_targets")]
    pub worker_targets: Vec<String>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub log_output: LogOutput,
    #[serde(default = "default_max_tenants")]
    pub(crate) max_tenants: i32,
    #[serde(default)]
    pub(crate) limits: Limits,
    #[serde(default)]
    pub(crate) http_timeouts: HttpTimeouts,
    #[serde(default)]
    pub(crate) mcp: Mcp,
    pub embedding: Embedding,
    #[serde(default)]
    pub(crate) rate_limit: RateLimit,
    #[serde(default)]
    pub(crate) db_load_guard: DbLoadGuard,
    #[serde(default)]
    pub query_embedding: QueryEmbedding,
    #[serde(default)]
    pub(crate) dispatch: Dispatch,
}

fn default_worker_targets() -> Vec<String> {
    vec!["worker-target:shared".into()]
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Dispatch {
    pub(crate) publish_concurrency: usize,
    pub(crate) poll_interval_ms: u64,
    pub(crate) claim_lease_seconds: u32,
    pub(crate) enqueue_timeout_ms: u64,
}

impl Default for Dispatch {
    fn default() -> Self {
        Self {
            publish_concurrency: 4,
            poll_interval_ms: 500,
            claim_lease_seconds: 30,
            enqueue_timeout_ms: 5_000,
        }
    }
}

/// Operator-tunable defaults that used to be fixed in code.
/// Every field has a default so a configuration file written before this section existed keeps loading.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Limits {
    /// Lifetime of an invite minted by the `create_invite` task when `ttl_hours` is omitted.
    pub(crate) invite_ttl_hours: i64,
    /// `Retry-After` advertised while maintenance is on, when the caller does not name one.
    pub(crate) maintenance_retry_after_seconds: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            invite_ttl_hours: 72,
            maintenance_retry_after_seconds: 300,
        }
    }
}

/// Deadlines for outbound calls, in seconds.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct HttpTimeouts {
    /// One embedding request to an OpenAI-compatible provider.
    pub(crate) embedding_request_seconds: u64,
    /// One batch through the in-process local embedding model.
    pub(crate) local_embedding_seconds: u64,
    /// Connecting to the host that serves the local model files.
    pub(crate) model_fetch_connect_seconds: u64,
    /// Reading from it.
    pub(crate) model_fetch_read_seconds: u64,
    /// One inference request to a workspace's language model.
    /// Longer than an embedding call: a chat completion over several fields is slower, and the work already runs behind a job id.
    pub(crate) inference_request_seconds: u64,
    /// One OIDC discovery or token call.
    pub(crate) oauth_discovery_seconds: u64,
}

impl Default for HttpTimeouts {
    fn default() -> Self {
        Self {
            embedding_request_seconds: 30,
            local_embedding_seconds: 30,
            model_fetch_connect_seconds: 30,
            model_fetch_read_seconds: 60,
            inference_request_seconds: 120,
            oauth_discovery_seconds: 10,
        }
    }
}

fn default_timezone() -> String {
    "UTC".into()
}

fn default_max_tenants() -> i32 {
    1
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Mcp {
    /// Comma-separated Host authorities accepted in addition to loopback and the bind address.
    pub(crate) allowed_hosts: Option<String>,
}

/// How the API and MCP server waits for a worker to embed a search query.
///
/// The server holds no embedding model: it enqueues a request and polls for the result, so these three values bound that wait.
/// The section is optional so a configuration file written before it existed keeps loading with the defaults.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
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

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Embedding {
    pub provider: EmbeddingProvider,
    pub(crate) dimensions: usize,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: String,
    pub(crate) model: Option<String>,
    pub(crate) send_dimensions_param: bool,
    #[serde(default)]
    pub(crate) query_instruction: String,
    #[serde(default)]
    pub(crate) document_prefix: String,
    pub(crate) local_model: String,
    #[serde(default = "default_local_max_sequence_length")]
    pub(crate) local_max_sequence_length: usize,
    /// Maximum concurrent provider calls in one worker process.
    #[serde(default = "default_provider_concurrency")]
    pub(crate) provider_concurrency: usize,
}

fn default_local_max_sequence_length() -> usize {
    512
}

fn default_provider_concurrency() -> usize {
    1
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    None,
    Local,
    Openai,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
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

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct DbLoadGuard {
    /// Zero disables the guard.
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

/// The deployment settings of this application context.
///
/// The context's config stays the source of truth: a context whose config was replaced after
/// boot (a second application in one process, a reconfigured test context) reads its own values.
///
/// # Errors
/// Returns an error when the config carries no valid `settings` block.
pub(crate) fn configured(ctx: &loco_rs::app::AppContext) -> loco_rs::Result<Settings> {
    ctx.config.settings::<Settings>()
}
