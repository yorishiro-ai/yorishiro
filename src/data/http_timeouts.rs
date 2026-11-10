//! Outbound HTTP and inference deadlines.
//!
//! The deadlines come from the deployment settings and travel with the client that honors them:
//! every provider and fetcher receives the timeouts of the context that built it, so a process
//! booting several application contexts never mixes one context's deadlines into another's clients.
//! A client built without deployment settings (a tool, a test) uses the documented defaults.

use std::time::Duration;

use crate::data::settings::HttpTimeouts;

/// Which outbound call a deadline bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpTimeout {
    EmbeddingRequest,
    LocalEmbedding,
    ModelFetchConnect,
    ModelFetchRead,
    InferenceRequest,
    OidcDiscovery,
}

/// The deadline for `kind` under an explicit set of deployment timeouts.
#[must_use]
pub(crate) fn deadline_for(kind: HttpTimeout, timeouts: HttpTimeouts) -> Duration {
    Duration::from_secs(match kind {
        HttpTimeout::EmbeddingRequest => timeouts.embedding_request_seconds,
        HttpTimeout::LocalEmbedding => timeouts.local_embedding_seconds,
        HttpTimeout::ModelFetchConnect => timeouts.model_fetch_connect_seconds,
        HttpTimeout::ModelFetchRead => timeouts.model_fetch_read_seconds,
        HttpTimeout::InferenceRequest => timeouts.inference_request_seconds,
        HttpTimeout::OidcDiscovery => timeouts.oauth_discovery_seconds,
    })
}
