//! Process-wide outbound HTTP and inference deadlines.
//!
//! The HTTP clients are shared per process and built lazily, often from paths that hold no `AppContext`, so the deadlines are installed once at start-up instead of being threaded through every constructor.
//! A process that never installed them (a unit test, a tool) uses the documented defaults.

use std::sync::OnceLock;
use std::time::Duration;

use crate::data::settings::HttpTimeouts;

static ACTIVE: OnceLock<HttpTimeouts> = OnceLock::new();

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

/// The first call wins: Loco builds the context twice for `start`, and both builds read the same file.
pub(crate) fn install(timeouts: HttpTimeouts) {
    let _ = ACTIVE.set(timeouts);
}

/// The configured deadline for `kind`, or its default when none was installed.
#[must_use]
pub fn deadline(kind: HttpTimeout) -> Duration {
    let t = ACTIVE.get().copied().unwrap_or_default();
    Duration::from_secs(match kind {
        HttpTimeout::EmbeddingRequest => t.embedding_request_seconds,
        HttpTimeout::LocalEmbedding => t.local_embedding_seconds,
        HttpTimeout::ModelFetchConnect => t.model_fetch_connect_seconds,
        HttpTimeout::ModelFetchRead => t.model_fetch_read_seconds,
        HttpTimeout::InferenceRequest => t.inference_request_seconds,
        HttpTimeout::OidcDiscovery => t.oauth_discovery_seconds,
    })
}
