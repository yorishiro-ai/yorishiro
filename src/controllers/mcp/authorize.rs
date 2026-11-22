//! Authenticating a request's bearer key and authorizing each tool call against its scope and the active policy.

use super::{
    ApiKeyScope, AppContext, AuthContext, CallToolResult, ErrorData, Parts, YorishiroError, auth,
    err_to_tool_result,
};

/// The outcome of authenticating a tool call and checking its scope, before any transaction is opened.
pub(crate) enum VerifyOutcome {
    Verified(AuthContext),
    ScopeDenied(CallToolResult),
}

/// Copies the request's headers into the shape `auth::Authenticator` takes: the same thing the REST adapter does, so a replaced authenticator sees an MCP call exactly as it sees a REST one.
pub(super) fn extract_bearer_key(parts: &Parts) -> Result<&str, ErrorData> {
    auth::extract_bearer_key(&parts.headers).ok_or_else(|| {
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

/// Authenticates and checks scope before resolving an operation target.
pub(crate) async fn verify(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<VerifyOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = auth::header_pairs(&parts.headers).map_err(header_error)?;

    match auth::RequestAuth::new(ctx)
        .resolve(presented_key, &headers)
        .await
    {
        Ok(current) => {
            if let Err(error) = auth::require_scope(&current, required) {
                return Ok(VerifyOutcome::ScopeDenied(scope_denied(
                    error,
                    required,
                    current.scope,
                )));
            }
            crate::models::api_keys::Entity::touch_authenticated(ctx, &current)
                .await
                .map_err(crate::error::mcp_internal)?;
            Ok(VerifyOutcome::Verified(current))
        }
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(VerifyOutcome::ScopeDenied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(YorishiroError::ValidationFailed { message, .. }) => {
            Err(ErrorData::invalid_params(message, None))
        }
        Err(err) => Err(crate::error::mcp_internal(err)),
    }
}

fn scope_denied(
    error: YorishiroError,
    required: ApiKeyScope,
    effective: ApiKeyScope,
) -> CallToolResult {
    let mut result = err_to_tool_result(error);
    if let Some(details) = result
        .structured_content
        .as_mut()
        .and_then(|value| value.get_mut("error"))
    {
        details["required_scope"] = serde_json::json!(required);
        details["effective_scope"] = serde_json::json!(effective);
    }
    result
}
