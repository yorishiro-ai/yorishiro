//! Process-wide outbound HTTP and inference deadlines.
//!
//! The HTTP clients are shared per process and built lazily, often from paths that hold no `AppContext`, so the deadlines are installed once at start-up instead of being threaded through every constructor.
//! A process that never installed them (a unit test, a tool) uses the documented defaults.

use std::sync::OnceLock;
use std::time::Duration;

use crate::data::settings::HttpTimeouts;

static ACTIVE: OnceLock<HttpTimeouts> = OnceLock::new();

/// The first call wins: Loco builds the context twice for `start`, and both builds read the same file.
pub(crate) fn install(timeouts: HttpTimeouts) {
    let _ = ACTIVE.set(timeouts);
}

fn active() -> HttpTimeouts {
    ACTIVE.get().copied().unwrap_or_default()
}

pub(crate) fn embedding_request() -> Duration {
    Duration::from_secs(active().embedding_request_seconds)
}

pub(crate) fn local_embedding() -> Duration {
    Duration::from_secs(active().local_embedding_seconds)
}

pub(crate) fn model_fetch_connect() -> Duration {
    Duration::from_secs(active().model_fetch_connect_seconds)
}

pub(crate) fn model_fetch_read() -> Duration {
    Duration::from_secs(active().model_fetch_read_seconds)
}

pub(crate) fn inference_request() -> Duration {
    Duration::from_secs(active().inference_request_seconds)
}

pub(crate) fn oauth_discovery() -> Duration {
    Duration::from_secs(active().oauth_discovery_seconds)
}
