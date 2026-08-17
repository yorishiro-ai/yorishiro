use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, thiserror::Error)]
pub enum YorishiroError {
    #[error("validation failed: {message}")]
    ValidationFailed {
        message: String,
        details: Vec<ValidationDetail>,
        hint: String,
    },

    #[error("not found: {message}")]
    NotFound { message: String },

    #[error("scope insufficient: {message}")]
    ScopeInsufficient { message: String, hint: String },

    #[error("conflict: {message}")]
    Conflict { message: String },

    #[error("relation type mismatch: {message}")]
    RelationTypeMismatch { message: String },

    #[error("unauthenticated")]
    Unauthenticated,

    /// The deployment is in maintenance.
    /// `read_only` refuses writes (423), `full_lock` refuses everything (503); `retry_after` is seconds, and reaches the caller as a header as well as in the body, since agents retry on the header.
    #[error("maintenance: {message}")]
    Maintenance {
        message: String,
        read_only: bool,
        retry_after: u32,
    },

    /// An upstream provider asked to be tried again later, rather than refusing outright.
    /// Carried separately from `Internal` so a caller can wait instead of discarding the work.
    #[error("provider busy: {message}")]
    ProviderBusy {
        message: String,
        retry_after: std::time::Duration,
    },

    /// An upstream provider could not be reached at all: connection refused, DNS failure, timeout.
    /// Separate from both `ProviderBusy` and `Internal` because it is the one of the three an operator can fix, and only if the response says so.
    ///
    /// A provider that answers `429` becomes `ProviderBusy` and reports the wait.
    /// A provider that cannot be reached at all maps here rather than to `Internal`, so the case with an actual remedy is distinguishable from the case without one: folding it into `Internal` would leave the log holding `error sending request for url ...` while the caller sees only `internal server error`.
    ///
    /// `url` is the configured base URL, not the caller's input, so naming it discloses nothing the deployment's own operator did not set.
    #[error("embedding provider unreachable at {url}: {message}")]
    ProviderUnreachable { url: String, message: String },

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl YorishiroError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }

    /// Maps this error to an HTTP status code and JSON response body.
    /// Every axum error wrapper built on `YorishiroError` (`yorishiro-server`'s `ApiError` and any downstream equivalent) delegates here so the status/body mapping is defined once and never duplicated as a second `match`.
    /// Internal errors are logged here (the caller should not log them again).
    pub fn into_http_parts(self) -> (u16, serde_json::Value) {
        match self {
            Self::ValidationFailed {
                message,
                details,
                hint,
            } => (
                422,
                serde_json::json!({ "error": { "message": message, "details": details, "hint": hint } }),
            ),
            Self::NotFound { message } => {
                (404, serde_json::json!({ "error": { "message": message } }))
            }
            Self::ScopeInsufficient { message, hint } => (
                403,
                serde_json::json!({ "error": { "message": message, "hint": hint } }),
            ),
            Self::Conflict { message } => {
                (409, serde_json::json!({ "error": { "message": message } }))
            }
            Self::RelationTypeMismatch { message } => {
                (422, serde_json::json!({ "error": { "message": message } }))
            }
            Self::Unauthenticated => (
                401,
                serde_json::json!({ "error": { "message": "authentication required" } }),
            ),
            Self::Maintenance {
                message,
                read_only,
                retry_after,
            } => (
                // 423 says "this resource is locked, the server is fine"; 503 says "the server is not serving".
                // Read-only is the first, full lock the second.
                if read_only { 423 } else { 503 },
                serde_json::json!({
                    "error": {
                        "message": message,
                        "retry_after_seconds": retry_after,
                    }
                }),
            ),
            // 503 rather than 500: the request may well succeed later, and the caller is told when.
            // Reaching a client at all is the unusual case: this normally surfaces in background embedding, where the retry happens without anyone seeing it.
            Self::ProviderBusy {
                message,
                retry_after,
            } => (
                503,
                serde_json::json!({
                    "error": {
                        "message": message,
                        "retry_after_seconds": retry_after.as_secs(),
                    }
                }),
            ),
            // 502 rather than 503: this deployment is up and answering, and the thing that is not is behind it.
            // 503 is already taken by `ProviderBusy`, where the provider did answer and asked for a wait.
            // A caller that retried this one on the same schedule would be waiting on a misconfiguration that no amount of waiting fixes.
            Self::ProviderUnreachable { url, message } => (
                502,
                serde_json::json!({
                    "error": {
                        "message": format!("the embedding provider at {url} could not be reached: {message}"),
                        "hint": "check that the provider is running and that YORISHIRO_EMBEDDING_BASE_URL points at it",
                    }
                }),
            ),
            Self::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (
                    500,
                    serde_json::json!({ "error": { "message": "internal server error" } }),
                )
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ValidationDetail {
    pub field: String,
    pub problem: String,
}

pub trait ResultExt<T> {
    fn internal(self) -> Result<T, YorishiroError>;
}

impl<T, E: Into<anyhow::Error>> ResultExt<T> for Result<T, E> {
    fn internal(self) -> Result<T, YorishiroError> {
        self.map_err(|err| YorishiroError::Internal(err.into()))
    }
}

#[cfg(test)]
#[path = "../tests/error.rs"]
mod tests;
