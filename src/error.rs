use serde::{Serialize, Serializer};

/// Every application-level error that flows to an HTTP response or tool result.
///
/// Adding a variant requires two things: a variant in this enum and a matching
/// arm in `YorishiroErrorCode::from_variant`.  A new variant without a code
/// fails to compile.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum YorishiroError {
    /// The request body or parameters do not conform to the expected schema.
    /// Includes per-field [`ValidationDetail`] so clients can render precise
    /// messages.
    #[error("validation failed: {message}")]
    ValidationFailed {
        message: String,
        details: Vec<ValidationDetail>,
        hint: String,
    },

    /// The requested resource does not exist.
    #[error("not found: {message}")]
    NotFound { message: String },

    /// The authenticated principal lacks the required scope or role.
    #[error("scope insufficient: {message}")]
    ScopeInsufficient { message: String, hint: String },

    /// The request would create a duplicate or contradict an existing resource.
    #[error("conflict: {message}")]
    Conflict { message: String },

    /// The relation type does not match the expected category.
    #[error("relation type mismatch: {message}")]
    RelationTypeMismatch { message: String },

    /// No valid authentication credentials were provided.
    #[error("unauthenticated")]
    Unauthenticated,

    /// The deployment is in maintenance mode.
    ///
    /// `read_only` refuses writes (423), `full_lock` refuses everything (503);
    /// `retry_after` is seconds, and reaches the caller as a header as well as
    /// in the body, since agents retry on the header.
    #[error("maintenance: {message}")]
    Maintenance {
        message: String,
        read_only: bool,
        retry_after: u32,
    },

    /// The backend provider (e.g. LLM, embedding) responded with a busy / retry-after signal.
    #[error("provider busy: {message}")]
    ProviderBusy {
        message: String,
        retry_after: std::time::Duration,
    },

    /// The embedding provider HTTP endpoint is unreachable.
    #[error("embedding provider unreachable at {url}: {message}")]
    ProviderUnreachable { url: String, message: String },

    /// The requested feature is not supported on the current backend.
    #[error("not implemented for backend: {message}")]
    BackendUnsupported { message: String },

    /// A required backend component (queue provider, embedding provider) is not configured.
    ///
    /// Distinct from `ProviderUnreachable` (502, provider is unreachable) and
    /// `ProviderBusy` (503, provider is busy): this variant covers the "not
    /// configured at all" case where no backend component exists to contact or
    /// wait for.
    #[error("backend unavailable: {message}")]
    BackendUnavailable { message: String },

    /// An unexpected internal error occurred.
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
} for every `YorishiroError` variant.
///
/// This is the **single source of truth**: the code, HTTP status, and
/// description are all defined on this enum.  Looking up the code tells you
/// everything about the error — status code, meaning, and typical response.
///
/// ## Code reference table
///
/// | Code | HTTP | Meaning |
/// |---|---|---|
/// | `validation_failed` | 422 | Request body does not conform to schema |
/// | `not_found` | 404 | Resource does not exist |
/// | `scope_insufficient` | 403 | Insufficient permissions |
/// | `conflict` | 409 | Duplicate or contradicting request |
/// | `relation_type_mismatch` | 422 | Relation type does not match category |
/// | `unauthenticated` | 401 | Authentication required |
/// | `maintenance_read_only` | 423 | Read-only maintenance mode |
/// | `maintenance_full_lock` | 503 | Full maintenance lock |
/// | `provider_busy` | 503 | Backend provider temporarily busy |
/// | `provider_unreachable` | 502 | Embedding provider unreachable |
/// | `backend_unsupported` | 501 | Feature not supported on this backend |
/// | `backend_unavailable` | 503 | Required backend not configured |
///
/// Internal errors have no code (they are not listed here) — callers must
/// handle `YorishiroError::Internal` separately and log the real error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum YorishiroErrorCode {
    /// 422 Unprocessable Entity — the request body does not conform to the expected schema.
    ValidationFailed,
    /// 404 Not Found — the requested resource does not exist.
    NotFound,
    /// 403 Forbidden — the authenticated principal lacks the required scope or role.
    ScopeInsufficient,
    /// 409 Conflict — the request would create a duplicate or contradict an existing resource.
    Conflict,
    /// 422 Unprocessable Entity — the relation type does not match the expected category.
    RelationTypeMismatch,
    /// 401 Unauthorized — no valid authentication credentials were provided.
    Unauthenticated,
    /// 423 Locked — the deployment is in read-only maintenance mode.
    MaintenanceReadOnly,
    /// 503 Service Unavailable — the deployment is fully locked for maintenance.
    MaintenanceFullLock,
    /// 503 Service Unavailable — the backend provider is temporarily busy.
    ProviderBusy,
    /// 502 Bad Gateway — the embedding provider is unreachable.
    ProviderUnreachable,
    /// 501 Not Implemented — the requested feature is not supported on this backend.
    BackendUnsupported,
    /// 503 Service Unavailable — a required backend component is not configured.
    BackendUnavailable,
}

impl Serialize for YorishiroErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl YorishiroErrorCode {
    /// Machine-readable string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ValidationFailed => "validation_failed",
            Self::NotFound => "not_found",
            Self::ScopeInsufficient => "scope_insufficient",
            Self::Conflict => "conflict",
            Self::RelationTypeMismatch => "relation_type_mismatch",
            Self::Unauthenticated => "unauthenticated",
            Self::MaintenanceReadOnly => "maintenance_read_only",
            Self::MaintenanceFullLock => "maintenance_full_lock",
            Self::ProviderBusy => "provider_busy",
            Self::ProviderUnreachable => "provider_unreachable",
            Self::BackendUnsupported => "backend_unsupported",
            Self::BackendUnavailable => "backend_unavailable",
        }
    }

    /// HTTP status code for this error code.
    ///
    /// Status codes are fixed by the HTTP specification and never change.
    pub fn http_status(&self) -> axum::http::StatusCode {
        match self {
            Self::ValidationFailed => axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound => axum::http::StatusCode::NOT_FOUND,
            Self::ScopeInsufficient => axum::http::StatusCode::FORBIDDEN,
            Self::Conflict => axum::http::StatusCode::CONFLICT,
            Self::RelationTypeMismatch => axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthenticated => axum::http::StatusCode::UNAUTHORIZED,
            Self::MaintenanceReadOnly => axum::http::StatusCode::LOCKED,
            Self::MaintenanceFullLock => axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Self::ProviderBusy => axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Self::ProviderUnreachable => axum::http::StatusCode::BAD_GATEWAY,
            Self::BackendUnsupported => axum::http::StatusCode::NOT_IMPLEMENTED,
            Self::BackendUnavailable => axum::http::StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Human-readable one-line description of the error.
    pub fn description(&self) -> &'static str {
        match self {
            Self::ValidationFailed => "The request body does not conform to the expected schema",
            Self::NotFound => "The requested resource does not exist",
            Self::ScopeInsufficient => "Insufficient permissions for the requested operation",
            Self::Conflict => "The request conflicts with an existing resource",
            Self::RelationTypeMismatch => "The relation type does not match the expected category",
            Self::Unauthenticated => "Authentication is required",
            Self::MaintenanceReadOnly => "The deployment is in read-only maintenance mode",
            Self::MaintenanceFullLock => "The deployment is fully locked for maintenance",
            Self::ProviderBusy => "The backend provider is temporarily busy",
            Self::ProviderUnreachable => "The embedding provider is unreachable",
            Self::BackendUnsupported => "The requested feature is not supported on this backend",
            Self::BackendUnavailable => "A required backend component is not configured",
        }
    }
}

impl YorishiroError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }

    /// Returns the [`YorishiroErrorCode`] for this error variant.
    ///
    /// `Internal` has no code (it is not listed in `YorishiroErrorCode`),
    /// so this returns `None`. Callers must handle `Internal` separately.
    pub fn code(&self) -> Option<YorishiroErrorCode> {
        match self {
            Self::ValidationFailed { .. } => Some(YorishiroErrorCode::ValidationFailed),
            Self::NotFound { .. } => Some(YorishiroErrorCode::NotFound),
            Self::ScopeInsufficient { .. } => Some(YorishiroErrorCode::ScopeInsufficient),
            Self::Conflict { .. } => Some(YorishiroErrorCode::Conflict),
            Self::RelationTypeMismatch { .. } => Some(YorishiroErrorCode::RelationTypeMismatch),
            Self::Unauthenticated => Some(YorishiroErrorCode::Unauthenticated),
            Self::Maintenance { read_only, .. } => {
                if *read_only {
                    Some(YorishiroErrorCode::MaintenanceReadOnly)
                } else {
                    Some(YorishiroErrorCode::MaintenanceFullLock)
                }
            }
            Self::ProviderBusy { .. } => Some(YorishiroErrorCode::ProviderBusy),
            Self::ProviderUnreachable { .. } => Some(YorishiroErrorCode::ProviderUnreachable),
            Self::BackendUnsupported { .. } => Some(YorishiroErrorCode::BackendUnsupported),
            Self::BackendUnavailable { .. } => Some(YorishiroErrorCode::BackendUnavailable),
            Self::Internal(_) => None,
        }
    }

    /// Maps this error to an HTTP status code and JSON response body.
    ///
    /// The error code, HTTP status, and description all come from
    /// `YorishiroErrorCode` — there is exactly one place to look up or add
    /// an error definition.
    ///
    /// Internal errors are logged here; the caller should not log them again.
    pub fn into_http_parts(self) -> (axum::http::StatusCode, serde_json::Value) {
        match self {
            Self::ValidationFailed {
                message,
                details,
                hint,
            } => (
                YorishiroErrorCode::ValidationFailed.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::ValidationFailed.as_str(),
                        "message": message,
                        "details": details,
                        "hint": hint,
                    }
                }),
            ),
            Self::NotFound { message } => (
                YorishiroErrorCode::NotFound.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::NotFound.as_str(),
                        "message": message
                    }
                }),
            ),
            Self::ScopeInsufficient { message, hint } => (
                YorishiroErrorCode::ScopeInsufficient.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::ScopeInsufficient.as_str(),
                        "message": message,
                        "hint": hint
                    }
                }),
            ),
            Self::Conflict { message } => (
                YorishiroErrorCode::Conflict.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::Conflict.as_str(),
                        "message": message
                    }
                }),
            ),
            Self::RelationTypeMismatch { message } => (
                YorishiroErrorCode::RelationTypeMismatch.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::RelationTypeMismatch.as_str(),
                        "message": message
                    }
                }),
            ),
            Self::Unauthenticated => (
                YorishiroErrorCode::Unauthenticated.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::Unauthenticated.as_str(),
                        "message": "authentication required"
                    }
                }),
            ),
            Self::Maintenance {
                message,
                read_only,
                retry_after,
            } => {
                let code = if read_only {
                    YorishiroErrorCode::MaintenanceReadOnly
                } else {
                    YorishiroErrorCode::MaintenanceFullLock
                };
                (
                    code.http_status(),
                    serde_json::json!({
                        "error": {
                            "code": code.as_str(),
                            "message": message,
                            "retry_after_seconds": retry_after,
                        }
                    }),
                )
            }
            Self::ProviderBusy {
                message,
                retry_after,
            } => (
                YorishiroErrorCode::ProviderBusy.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::ProviderBusy.as_str(),
                        "message": message,
                        "retry_after_seconds": retry_after.as_secs(),
                    }
                }),
            ),
            Self::ProviderUnreachable { url, message } => (
                YorishiroErrorCode::ProviderUnreachable.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::ProviderUnreachable.as_str(),
                        "message": format!("the embedding provider at {url} could not be reached: {message}"),
                        "hint": "check that the provider is running and that YORISHIRO_EMBEDDING_BASE_URL points at it",
                    }
                }),
            ),
            Self::BackendUnsupported { message } => (
                YorishiroErrorCode::BackendUnsupported.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::BackendUnsupported.as_str(),
                        "message": message
                    }
                }),
            ),
            Self::BackendUnavailable { message } => (
                YorishiroErrorCode::BackendUnavailable.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::BackendUnavailable.as_str(),
                        "message": message
                    }
                }),
            ),
            Self::Internal(ref err) => {
                tracing::error!(error = %err, "internal error");
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({
                        "error": {
                            "code": "internal",
                            "message": "internal server error",
                            "hint": "this is an unexpected error; check server logs",
                        }
                    }),
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
        loco_rs::Error::CustomError(
            status,
            loco_rs::controller::ErrorDetail {
                error: Some(status.to_string()),
                description: None,
                errors: Some(body),
            },
        )
    }
}

/// Machine-readable classification of a validation error.
///
/// New branches in `src/models/schema_schemas/metaschema/validate.rs` should get their own variant here.
/// Use the `Other` catch-all for errors that do not fit a specific classification.
#[derive(Debug, Clone, Copy, Serialize)]
pub enum ValidationErrorCode {
    /// The field's type does not match the schema's expected type.
    TypeMismatch,
    /// A required field was empty or missing.
    EmptyRequired,
    /// Object nesting exceeds [`crate::models::schema_schemas::metaschema::MAX_OBJECT_DEPTH`].
    DepthExceeded,
    /// A numeric field uses minimum/maximum on a non-numeric type.
    NumericOnNonNumeric,
    /// A string-only constraint (format, min_length, max_length, pattern) is used on a non-string type.
    StringConstraintOnNonString,
    /// An array constraint (min_items, max_items, unique_items) is used on a non-array type.
    ArrayConstraintOnNonArray,
    /// An array items.type is neither 'string' nor 'object'.
    InvalidArrayItemType,
    /// An array with items.type = 'object' has empty or missing properties.
    EmptyObjectProperties,
    /// A field uses minimum > maximum.
    MinExceedsMax,
    /// An invalid regular expression pattern was provided.
    InvalidPattern,
    /// A format value is not one of the allowed values.
    UnsupportedFormat,
    /// A relation references an entity type that is not defined.
    UndefinedEntityType,
    /// Catch-all: a validation branch that does not match a specific variant.
    Other,
}

/// A single validation error detail, included in `ValidationFailed.details`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ValidationDetail {
    /// JSON pointer to the field being validated (e.g. `/entity_types/foo/fields/bar`).
    pub field: String,
    /// A human-readable description of the problem.
    /// Serves as the fallback/log string when no localized message is available.
    pub problem: String,
    /// Machine-readable classification of the error.
    /// Clients should switch on this to render localized messages.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub code: ValidationErrorCode,
    /// The expected value (e.g. "string", "object", "5"), or `None` if not applicable.
    pub expected: Option<String>,
    /// The actual value observed (e.g. "number", "array", "10"), or `None` if not applicable.
    pub actual: Option<String>,
}

pub trait ResultExt<T> {
    fn internal(self) -> Result<T, YorishiroError>;
}

impl<T, E: Into<anyhow::Error>> ResultExt<T> for Result<T, E> {
    fn internal(self) -> Result<T, YorishiroError> {
        self.map_err(|err| YorishiroError::Internal(err.into()))
    }
}
