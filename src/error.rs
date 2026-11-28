//! Application errors and their rendering policy at REST, MCP and Loco boundaries.
//!
//! Internal details are logged once at the boundary, never returned to clients.

use serde::{Serialize, Serializer};

/// The payload of an internal failure: any error, or a message.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `Result` with a [`BoxError`], for fallible setup code outside the request path.
pub type BoxResult<T> = Result<T, BoxError>;

/// Every application-level error that flows to an HTTP response or tool result.
///
/// Adding a variant requires two things: a variant in this enum and a matching
/// arm in `YorishiroErrorCode::from_variant`.  A new variant without a code
/// fails to compile.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
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

    #[error("multiple workspaces permit this operation")]
    WorkspaceSelectionRequired { workspaces: Vec<uuid::Uuid> },
    #[error("no workspace permits this operation")]
    WorkspaceAccessDenied,

    /// The requested resource does not exist.
    #[error("not found: {message}")]
    NotFound { message: String },

    /// The authenticated principal does not have permission for the operation.
    #[error("access denied: {message}")]
    AccessDenied { message: String, hint: String },

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

    /// The provider HTTP endpoint is unreachable or does not respond as expected.
    #[error("provider unreachable at {url}: {message}")]
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
    Internal(#[source] BoxError),
}

impl From<BoxError> for YorishiroError {
    fn from(error: BoxError) -> Self {
        Self::Internal(error)
    }
}

/// Reports an internal failure at its transport boundary and returns the safe client message.
pub(crate) fn report_internal(error: &BoxError) -> &'static str {
    tracing::error!(error = %error, "internal error");
    "internal server error"
}

/// Applies the same internal-error redaction to MCP protocol failures as REST and tool results.
pub(crate) fn mcp_internal(error: impl Into<BoxError>) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(report_internal(&error.into()), None)
}

/// Machine-readable error code for every `YorishiroError` variant.
///
/// This is the **single source of truth**: the code, HTTP status and description are all defined on this enum.
///
/// ## Code reference table
///
/// | Code | HTTP | Meaning |
/// |---|---|---|
/// | `validation_failed` | 422 | Request body does not conform to schema |
/// | `not_found` | 404 | Resource does not exist |
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
/// Internal errors have no code, so callers handle `YorishiroError::Internal` separately and log the real error.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum YorishiroErrorCode {
    /// 422 Unprocessable Entity: the request body does not conform to the expected schema.
    ValidationFailed,
    WorkspaceSelectionRequired,
    AccessDenied,
    /// 404 Not Found: the requested resource does not exist.
    NotFound,
    /// 403 Forbidden: the authenticated principal lacks the required scope or role.
    /// 409 Conflict: the request would create a duplicate or contradict an existing resource.
    Conflict,
    /// 422 Unprocessable Entity: the relation type does not match the expected category.
    RelationTypeMismatch,
    /// 401 Unauthorized: no valid authentication credentials were provided.
    Unauthenticated,
    /// 423 Locked: the deployment is in read-only maintenance mode.
    MaintenanceReadOnly,
    /// 503 Service Unavailable: the deployment is fully locked for maintenance.
    MaintenanceFullLock,
    /// 503 Service Unavailable: the backend provider is temporarily busy.
    ProviderBusy,
    /// 502 Bad Gateway: the embedding provider is unreachable.
    ProviderUnreachable,
    /// 501 Not Implemented: the requested feature is not supported on this backend.
    BackendUnsupported,
    /// 503 Service Unavailable: a required backend component is not configured.
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
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ValidationFailed => "validation_failed",
            Self::WorkspaceSelectionRequired => "workspace_selection_required",
            Self::AccessDenied => "access_denied",
            Self::NotFound => "not_found",
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
    #[must_use]
    pub const fn http_status(self) -> axum::http::StatusCode {
        match self {
            Self::ValidationFailed
            | Self::RelationTypeMismatch
            | Self::WorkspaceSelectionRequired => axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound => axum::http::StatusCode::NOT_FOUND,
            Self::AccessDenied => axum::http::StatusCode::FORBIDDEN,
            Self::Conflict => axum::http::StatusCode::CONFLICT,
            Self::Unauthenticated => axum::http::StatusCode::UNAUTHORIZED,
            Self::MaintenanceReadOnly => axum::http::StatusCode::LOCKED,
            Self::MaintenanceFullLock | Self::ProviderBusy | Self::BackendUnavailable => {
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            }
            Self::ProviderUnreachable => axum::http::StatusCode::BAD_GATEWAY,
            Self::BackendUnsupported => axum::http::StatusCode::NOT_IMPLEMENTED,
        }
    }

    /// Human-readable one-line description of the error.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::ValidationFailed => "The request body does not conform to the expected schema",
            Self::WorkspaceSelectionRequired => "Select one of the permitted workspaces",
            Self::AccessDenied => "Insufficient permissions for the requested operation",
            Self::NotFound => "The requested resource does not exist",
            Self::Conflict => "The request conflicts with an existing resource",
            Self::RelationTypeMismatch => "The relation type does not match the expected category",
            Self::Unauthenticated => "Authentication is required",
            Self::MaintenanceReadOnly => "The deployment is in read-only maintenance mode",
            Self::MaintenanceFullLock => "The deployment is fully locked for maintenance",
            Self::ProviderBusy => "The backend provider is temporarily busy",
            Self::ProviderUnreachable => "The configured provider is unreachable",
            Self::BackendUnsupported => "The requested feature is not supported on this backend",
            Self::BackendUnavailable => "A required backend component is not configured",
        }
    }
    fn message_response(
        self,
        message: impl Into<String>,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        (
            self.http_status(),
            serde_json::json!({
                "error": { "code": self.as_str(), "message": message.into() }
            }),
        )
    }
}

impl YorishiroError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }

    /// The classification the audit trail keeps: the error's code, or `internal` for an unexpected failure.
    #[must_use]
    pub const fn trail_code(&self) -> &'static str {
        match self.code() {
            Some(code) => code.as_str(),
            None => "internal",
        }
    }

    /// A description that never repeats input: the code's fixed text, plus the names of the fields a validation refused.
    #[must_use]
    pub fn trail_message(&self) -> String {
        match (self, self.code()) {
            (Self::ValidationFailed { details, .. }, Some(code)) if !details.is_empty() => {
                let fields: Vec<&str> =
                    details.iter().map(|detail| detail.field.as_str()).collect();
                format!("{}: {}", code.description(), fields.join(", "))
            }
            (_, Some(code)) => code.description().to_owned(),
            (_, None) => "internal error".to_owned(),
        }
    }

    /// The HTTP status the error maps to, so a transport without one can still report it.
    #[must_use]
    pub const fn trail_status(&self) -> u16 {
        match self.code() {
            Some(code) => code.http_status().as_u16(),
            None => 500,
        }
    }

    /// Returns the [`YorishiroErrorCode`] for this error variant.
    ///
    /// `Internal` has no code (it is not listed in `YorishiroErrorCode`),
    /// so this returns `None`. Callers must handle `Internal` separately.
    #[must_use]
    pub const fn code(&self) -> Option<YorishiroErrorCode> {
        match self {
            Self::ValidationFailed { .. } => Some(YorishiroErrorCode::ValidationFailed),
            Self::WorkspaceSelectionRequired { .. } => {
                Some(YorishiroErrorCode::WorkspaceSelectionRequired)
            }
            Self::WorkspaceAccessDenied | Self::AccessDenied { .. } => {
                Some(YorishiroErrorCode::AccessDenied)
            }
            Self::NotFound { .. } => Some(YorishiroErrorCode::NotFound),
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
    /// The error code, HTTP status, and description all come from `YorishiroErrorCode`, so error definitions have one source.
    ///
    /// Internal errors are logged here; the caller should not log them again.
    #[expect(
        clippy::too_many_lines,
        reason = "one match defines the REST and MCP error envelopes"
    )]
    #[must_use]
    pub fn into_http_parts(self) -> (axum::http::StatusCode, serde_json::Value) {
        match self {
            Self::WorkspaceSelectionRequired { workspaces } => (
                YorishiroErrorCode::WorkspaceSelectionRequired.http_status(),
                serde_json::json!({"error": {
                    "code": YorishiroErrorCode::WorkspaceSelectionRequired.as_str(),
                    "message": "multiple workspaces permit this operation",
                    "details": workspaces.iter().map(|id| serde_json::json!({
                        "field": "workspace_id",
                        "problem": id.to_string(),
                        "code": ValidationErrorCode::Other,
                    })).collect::<Vec<_>>(),
                    "hint": "select a workspace with workspace_id or x-workspace-id; use discover_workspaces to see workspace names",
                }}),
            ),
            Self::WorkspaceAccessDenied => YorishiroErrorCode::AccessDenied
                .message_response("no workspace permits this operation"),
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
            Self::NotFound { message } => YorishiroErrorCode::NotFound.message_response(message),
            Self::AccessDenied { message, hint } => (
                YorishiroErrorCode::AccessDenied.http_status(),
                serde_json::json!({
                    "error": {
                        "code": YorishiroErrorCode::AccessDenied.as_str(),
                        "message": message,
                        "hint": hint
                    }
                }),
            ),
            Self::Conflict { message } => YorishiroErrorCode::Conflict.message_response(message),
            Self::RelationTypeMismatch { message } => {
                YorishiroErrorCode::RelationTypeMismatch.message_response(message)
            }
            Self::Unauthenticated => {
                YorishiroErrorCode::Unauthenticated.message_response("authentication required")
            }
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
                        "message": format!("provider at {url}: {message}"),
                        "hint": "check that the provider is running and that its base URL, model and credentials are correct",
                    }
                }),
            ),
            Self::BackendUnsupported { message } => {
                YorishiroErrorCode::BackendUnsupported.message_response(message)
            }
            Self::BackendUnavailable { message } => {
                YorishiroErrorCode::BackendUnavailable.message_response(message)
            }
            Self::Internal(ref err) => {
                let message = report_internal(err);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({
                        "error": {
                            "code": "internal",
                            "message": message,
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
        Self::CustomError(
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
    /// A string-only constraint (format, `min_length`, `max_length`, pattern) is used on a non-string type.
    StringConstraintOnNonString,
    /// An array constraint (`min_items`, `max_items`, `unique_items`) is used on a non-array type.
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
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ValidationDetail {
    /// JSON pointer to the field being validated (e.g. `/entity_types/foo/fields/bar`).
    pub field: String,
    /// A human-readable description of the problem.
    /// Serves as the fallback/log string when no localized message is available.
    pub problem: String,
    /// Machine-readable classification of the error.
    /// Clients should switch on this to render localized messages.
    #[schemars(with = "String")]
    pub code: ValidationErrorCode,
    /// The expected value (e.g. "string", "object", "5"), or `None` if not applicable.
    pub expected: Option<String>,
    /// The actual value observed (e.g. "number", "array", "10"), or `None` if not applicable.
    pub actual: Option<String>,
}

impl YorishiroError {
    /// Whether the database refused a write because a row it references is missing or still referenced.
    /// `sea_orm` does not report PostgreSQL `restrict_violation` or a SQLite `RESTRICT` foreign key, so those are matched by code.
    #[must_use]
    pub fn is_foreign_key_violation(error: &sea_orm::DbErr) -> bool {
        use sea_orm::{DbErr, RuntimeErr};
        matches!(
            error,
            DbErr::Exec(RuntimeErr::SqlxError(sqlx_error))
                if sqlx_error.as_database_error().is_some_and(|database| {
                    database.code().as_deref() == Some("23001")
                        || (database.code().as_deref() == Some("1811")
                            && database.message() == "FOREIGN KEY constraint failed")
                })
        ) || matches!(
            error.sql_err(),
            Some(sea_orm::SqlErr::ForeignKeyConstraintViolation(_))
        )
    }

    /// A row that is gone (`RecordNotFound`, or `RecordNotUpdated` after a concurrent delete) becomes a not-found with `message`; any other database error is internal.
    /// Use it only where the missing row is what the caller named, never for an internal invariant such as a lost lease.
    #[must_use]
    pub fn not_found_on_missing(error: sea_orm::DbErr, message: impl Into<String>) -> Self {
        if matches!(
            error,
            sea_orm::DbErr::RecordNotFound(_) | sea_orm::DbErr::RecordNotUpdated
        ) {
            Self::NotFound {
                message: message.into(),
            }
        } else {
            Self::Internal(error.into())
        }
    }

    /// A foreign-key violation becomes a conflict with `message`; any other database error is internal.
    #[must_use]
    pub fn conflict_on_fk(error: sea_orm::DbErr, message: impl Into<String>) -> Self {
        if Self::is_foreign_key_violation(&error) {
            Self::Conflict {
                message: message.into(),
            }
        } else {
            Self::Internal(error.into())
        }
    }

    /// A unique-constraint violation becomes a conflict with `message`; any other database error is internal.
    pub(crate) fn conflict_on_unique(error: sea_orm::DbErr, message: impl Into<String>) -> Self {
        if matches!(
            error.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
        ) {
            Self::Conflict {
                message: message.into(),
            }
        } else {
            Self::Internal(error.into())
        }
    }
}

pub trait ResultExt<T> {
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    fn internal(self) -> Result<T, YorishiroError>;
}

impl<T, E: Into<BoxError>> ResultExt<T> for Result<T, E> {
    fn internal(self) -> Result<T, YorishiroError> {
        self.map_err(|err| YorishiroError::Internal(err.into()))
    }
}

/// Turns a missing optional value into the matching [`YorishiroError`].
pub trait OptionExt<T> {
    /// # Errors
    /// Returns `NotFound` with `message` when the value is absent.
    fn or_not_found(self, message: impl Into<String>) -> Result<T, YorishiroError>;

    /// # Errors
    /// Returns `Internal` with `message` when the value is absent.
    fn or_internal(self, message: &'static str) -> Result<T, YorishiroError>;
}

impl<T> OptionExt<T> for Option<T> {
    fn or_not_found(self, message: impl Into<String>) -> Result<T, YorishiroError> {
        self.ok_or_else(|| YorishiroError::not_found(message))
    }

    fn or_internal(self, message: &'static str) -> Result<T, YorishiroError> {
        self.ok_or_else(|| YorishiroError::Internal(message.into()))
    }
}

/// The trail's classification of an error that crossed into a Loco-owned path, such as a job failure.
///
/// Only the code and a fixed description are kept: the error's own text can repeat what the caller sent.
#[must_use]
pub(crate) fn describe_loco(error: &loco_rs::Error) -> (String, String) {
    let internal = || ("internal".to_owned(), "internal error".to_owned());
    match error {
        loco_rs::Error::Any(inner) => inner
            .downcast_ref::<YorishiroError>()
            .map_or_else(internal, |error| {
                (error.trail_code().to_owned(), error.trail_message())
            }),
        loco_rs::Error::CustomError(_, detail) => detail
            .errors
            .as_ref()
            .and_then(|body| body["error"]["code"].as_str())
            .map_or_else(internal, |code| {
                (code.to_owned(), detail.error.clone().unwrap_or_default())
            }),
        _ => internal(),
    }
}
