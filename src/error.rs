use serde::Serialize;

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

    #[error("provider busy: {message}")]
    ProviderBusy {
        message: String,
        retry_after: std::time::Duration,
    },

    #[error("embedding provider unreachable at {url}: {message}")]
    ProviderUnreachable { url: String, message: String },

    #[error("not implemented for backend: {message}")]
    BackendUnsupported { message: String },

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl YorishiroError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }

    /// A machine-readable identifier for this variant, stable across releases.
    /// Every variant must have one: a new variant with no arm here fails to compile.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ValidationFailed { .. } => "validation_failed",
            Self::NotFound { .. } => "not_found",
            Self::ScopeInsufficient { .. } => "scope_insufficient",
            Self::Conflict { .. } => "conflict",
            Self::RelationTypeMismatch { .. } => "relation_type_mismatch",
            Self::Unauthenticated => "unauthenticated",
            Self::Maintenance { read_only, .. } => {
                if *read_only {
                    "maintenance_read_only"
                } else {
                    "maintenance_full_lock"
                }
            }
            Self::ProviderBusy { .. } => "provider_busy",
            Self::ProviderUnreachable { .. } => "provider_unreachable",
            Self::BackendUnsupported { .. } => "backend_unsupported",
            Self::Internal(_) => "internal",
        }
    }

    /// Maps this error to an HTTP status code and JSON response body.
    /// Every axum error wrapper built on `YorishiroError` delegates here so the status/body mapping is defined once and never duplicated as a second `match`.
    /// Internal errors are logged here; the caller should not log them again.
    pub fn into_http_parts(self) -> (u16, serde_json::Value) {
        let code = self.code();
        match self {
            Self::ValidationFailed {
                message,
                details,
                hint,
            } => (
                422,
                serde_json::json!({ "error": { "code": code, "message": message, "details": details, "hint": hint } }),
            ),
            Self::NotFound { message } => (
                404,
                serde_json::json!({ "error": { "code": code, "message": message } }),
            ),
            Self::ScopeInsufficient { message, hint } => (
                403,
                serde_json::json!({ "error": { "code": code, "message": message, "hint": hint } }),
            ),
            Self::Conflict { message } => (
                409,
                serde_json::json!({ "error": { "code": code, "message": message } }),
            ),
            Self::RelationTypeMismatch { message } => (
                422,
                serde_json::json!({ "error": { "code": code, "message": message } }),
            ),
            Self::Unauthenticated => (
                401,
                serde_json::json!({ "error": { "code": code, "message": "authentication required" } }),
            ),
            Self::Maintenance {
                message,
                read_only,
                retry_after,
            } => (
                if read_only { 423 } else { 503 },
                serde_json::json!({
                    "error": {
                        "code": code,
                        "message": message,
                        "retry_after_seconds": retry_after,
                    }
                }),
            ),
            Self::ProviderBusy {
                message,
                retry_after,
            } => (
                503,
                serde_json::json!({
                    "error": {
                        "code": code,
                        "message": message,
                        "retry_after_seconds": retry_after.as_secs(),
                    }
                }),
            ),
            Self::ProviderUnreachable { url, message } => (
                502,
                serde_json::json!({
                    "error": {
                        "code": code,
                        "message": format!("the embedding provider at {url} could not be reached: {message}"),
                        "hint": "check that the provider is running and that YORISHIRO_EMBEDDING_BASE_URL points at it",
                    }
                }),
            ),
            Self::BackendUnsupported { message } => (
                501,
                serde_json::json!({ "error": { "code": code, "message": message } }),
            ),
            Self::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (
                    500,
                    serde_json::json!({ "error": { "code": code, "message": "internal server error" } }),
                )
            }
        }
    }
}

/// Lets a `YorishiroError` cross into a Loco-owned path (a `Hooks` method, a task, a worker) that returns `loco_rs::Result`.
/// Folds the whole `into_http_parts()` body into `ErrorDetail::errors`, since `ErrorDetail` has no dedicated `hint` field.
impl From<YorishiroError> for loco_rs::Error {
    fn from(err: YorishiroError) -> Self {
        let (status, body) = err.into_http_parts();
        let status_code = axum::http::StatusCode::from_u16(status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        loco_rs::Error::CustomError(
            status_code,
            loco_rs::controller::ErrorDetail {
                error: Some(status_code.to_string()),
                description: None,
                errors: Some(body),
            },
        )
    }
}

#[derive(Debug, Clone, Serialize)]
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
