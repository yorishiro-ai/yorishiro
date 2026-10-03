use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) max_tenants: i32,
    pub(crate) embedding: Embedding,
    pub(crate) rate_limit: RateLimit,
    pub(crate) db_load_guard: DbLoadGuard,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Embedding {
    pub(crate) provider: EmbeddingProvider,
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
pub(crate) enum EmbeddingProvider {
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

#[cfg(test)]
mod tests {
    use super::{EmbeddingProvider, Settings};

    fn settings() -> serde_json::Value {
        serde_json::json!({
            "max_tenants": 1,
            "embedding": {
                "provider": "local",
                "dimensions": 768,
                "base_url": null,
                "api_key": "",
                "model": null,
                "send_dimensions_param": false,
                "local_model": "multilingual-e5-base",
                "local_max_sequence_length": 512
            },
            "rate_limit": {
                "auth_max_requests": 10,
                "auth_window_seconds": 60,
                "search_tokens_per_minute": 100000
            },
            "db_load_guard": {
                "threshold": 0,
                "sustain_seconds": 30,
                "poll_seconds": 5
            }
        })
    }

    #[test]
    fn provider_is_typed_and_unknown_settings_are_rejected() {
        let parsed: Settings = serde_json::from_value(settings()).unwrap();
        assert_eq!(parsed.embedding.provider, EmbeddingProvider::Local);

        let mut unknown_provider = settings();
        unknown_provider["embedding"]["provider"] = "typo".into();
        assert!(serde_json::from_value::<Settings>(unknown_provider).is_err());

        let mut unknown_key = settings();
        unknown_key["rate_limit"]["obsolete"] = 1.into();
        assert!(serde_json::from_value::<Settings>(unknown_key).is_err());
    }
}
