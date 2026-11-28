//! Authenticating a request's bearer key; the active IAM policy authorizes each tool call.

use super::{
    AppContext, AuthContext, CallToolResult, ErrorData, Parts, YorishiroError, auth,
    err_to_tool_result,
};

/// The outcome of authenticating a tool call, before any transaction is opened.
pub(crate) enum VerifyOutcome {
    Verified(AuthContext),
    Denied(CallToolResult),
}

/// Requires a nonempty bearer credential on the request.
pub(super) fn extract_bearer_key(parts: &Parts) -> Result<&str, ErrorData> {
    auth::extract_bearer_key(&parts.headers).ok_or_else(|| {
        crate::controllers::middleware::access_log::denied();
        ErrorData::invalid_request("missing or malformed Authorization header", None)
    })
}

pub(super) fn header_error(error: YorishiroError) -> ErrorData {
    match error {
        YorishiroError::ValidationFailed { message, .. } => {
            ErrorData::invalid_params(message, None)
        }
        error => crate::error::mcp_internal(error),
    }
}

/// Authenticates before resolving an operation target.
pub(crate) async fn verify(ctx: &AppContext, parts: &Parts) -> Result<VerifyOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = auth::header_pairs(&parts.headers).map_err(header_error)?;

    match auth::RequestAuth::new(ctx)
        .resolve(presented_key, &headers)
        .await
        .inspect_err(|error| {
            tracing::warn!(%error, "MCP request rejected during authentication");
        }) {
        Ok(current) => {
            crate::models::access_keys::Entity::touch_authenticated(ctx, &current)
                .await
                .map_err(crate::error::mcp_internal)?;
            Ok(VerifyOutcome::Verified(current))
        }
        Err(err @ (YorishiroError::AccessDenied { .. } | YorishiroError::NotFound { .. })) => {
            Ok(VerifyOutcome::Denied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            crate::controllers::middleware::access_log::denied();
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(YorishiroError::ValidationFailed { message, .. }) => {
            Err(ErrorData::invalid_params(message, None))
        }
        Err(err) => Err(crate::error::mcp_internal(err)),
    }
}
