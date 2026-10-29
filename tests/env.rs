//! Runtime environment inputs owned by the integration-test harness.

use std::ffi::OsString;

#[derive(Clone, Copy)]
pub(crate) enum TestEnv {
    WorkerTargets,
    WorkerOfficialLimit,
    WorkerClassTenantPrivateLimit,
    WorkerClassSharedLimit,
    Timezone,
    TestTopology,
    #[cfg(feature = "enterprise")]
    StripeWebhookSecret,
    #[cfg(feature = "enterprise")]
    StripePriceTeam,
    #[cfg(feature = "enterprise")]
    StripePricePro,
    QueueWorkers,
    QueueKind,
    QueryEmbeddingTimeoutMs,
    QueryEmbeddingRetentionSecs,
    QueryEmbeddingPollIntervalMs,
    PlanPolicyEnabled,
    #[cfg(feature = "enterprise")]
    OauthIssuerUrl,
    #[cfg(feature = "enterprise")]
    OauthClientSecret,
    #[cfg(feature = "enterprise")]
    OauthClientId,
    McpAllowedHosts,
    MaxTenants,
    LocalModel,
    LicenseKey,
    EmbeddingProvider,
    AllowedOrigins,
    QueueUrl,
    Port,
    LocoConfigFolder,
    DbMaxConnections,
    DbConnectTimeout,
    Binding,
    DatabaseUrl,
    LocoEnvironment,
    TopologyMarkerDir,
    BackendMarkerDir,
    RequestTimingDir,
    #[cfg(feature = "enterprise")]
    SqliteLockChildUri,
    RedisBaseUrl,
    E5Model,
    E5Tokenizer,
}

impl TestEnv {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::WorkerTargets => "YORISHIRO_WORKER_TARGETS",
            Self::WorkerOfficialLimit => "YORISHIRO_WORKER_OFFICIAL_LIMIT",
            Self::WorkerClassTenantPrivateLimit => "YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT",
            Self::WorkerClassSharedLimit => "YORISHIRO_WORKER_CLASS_SHARED_LIMIT",
            Self::Timezone => "YORISHIRO_TIMEZONE",
            Self::TestTopology => "YORISHIRO_TEST_TOPOLOGY",
            #[cfg(feature = "enterprise")]
            Self::StripeWebhookSecret => "YORISHIRO_STRIPE_WEBHOOK_SECRET",
            #[cfg(feature = "enterprise")]
            Self::StripePriceTeam => "YORISHIRO_STRIPE_PRICE_TEAM",
            #[cfg(feature = "enterprise")]
            Self::StripePricePro => "YORISHIRO_STRIPE_PRICE_PRO",
            Self::QueueWorkers => "YORISHIRO_QUEUE_WORKERS",
            Self::QueueKind => "YORISHIRO_QUEUE_KIND",
            Self::QueryEmbeddingTimeoutMs => "YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS",
            Self::QueryEmbeddingRetentionSecs => "YORISHIRO_QUERY_EMBEDDING_RETENTION_SECS",
            Self::QueryEmbeddingPollIntervalMs => "YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS",
            Self::PlanPolicyEnabled => "YORISHIRO_PLAN_POLICY_ENABLED",
            #[cfg(feature = "enterprise")]
            Self::OauthIssuerUrl => "YORISHIRO_OAUTH_ISSUER_URL",
            #[cfg(feature = "enterprise")]
            Self::OauthClientSecret => "YORISHIRO_OAUTH_CLIENT_SECRET",
            #[cfg(feature = "enterprise")]
            Self::OauthClientId => "YORISHIRO_OAUTH_CLIENT_ID",
            Self::McpAllowedHosts => "YORISHIRO_MCP_ALLOWED_HOSTS",
            Self::MaxTenants => "YORISHIRO_MAX_TENANTS",
            Self::LocalModel => "YORISHIRO_LOCAL_MODEL",
            Self::LicenseKey => "YORISHIRO_LICENSE_KEY",
            Self::EmbeddingProvider => "YORISHIRO_EMBEDDING_PROVIDER",
            Self::AllowedOrigins => "YORISHIRO_ALLOWED_ORIGINS",
            Self::QueueUrl => "QUEUE_URL",
            Self::Port => "PORT",
            Self::LocoConfigFolder => "LOCO_CONFIG_FOLDER",
            Self::DbMaxConnections => "DB_MAX_CONNECTIONS",
            Self::DbConnectTimeout => "DB_CONNECT_TIMEOUT",
            Self::Binding => "BINDING",
            Self::DatabaseUrl => "DATABASE_URL",
            Self::LocoEnvironment => "LOCO_ENV",
            Self::TopologyMarkerDir => "YORISHIRO_TOPOLOGY_MARKER_DIR",
            Self::BackendMarkerDir => "YORISHIRO_BACKEND_MARKER_DIR",
            Self::RequestTimingDir => "YORISHIRO_REQUEST_TIMING_DIR",
            #[cfg(feature = "enterprise")]
            Self::SqliteLockChildUri => "YORISHIRO_SQLITE_LOCK_CHILD_URI",
            Self::RedisBaseUrl => "YORISHIRO_TEST_REDIS_BASE_URL",
            Self::E5Model => "YORISHIRO_TEST_E5_MODEL",
            Self::E5Tokenizer => "YORISHIRO_TEST_E5_TOKENIZER",
        }
    }

    pub(crate) fn read(self) -> Result<String, std::env::VarError> {
        std::env::var(self.name())
    }

    pub(crate) fn read_os(self) -> Option<OsString> {
        std::env::var_os(self.name())
    }
}
