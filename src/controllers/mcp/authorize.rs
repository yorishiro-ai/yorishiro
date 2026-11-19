//! Authenticating a request's bearer key and authorizing each tool call against its scope and the active policy.

use super::*;

/// Auth context plus a transaction with RLS already configured, held by calls that passed authentication and scope checks.
pub(crate) struct Authorized {
    ctx: AuthContext,
    txn: DatabaseTransaction,
    identity: sea_orm::DatabaseConnection,
    authorization: Option<DatabaseTransaction>,
    required: ApiKeyScope,
    workspace_id: uuid::Uuid,
}

impl Authorized {
    pub(crate) fn txn(&self) -> &DatabaseTransaction {
        &self.txn
    }

    /// Commits the transaction.
    /// Every write handler must call this before returning `Ok`.
    pub(crate) async fn commit(self) -> Result<(), ErrorData> {
        self.txn
            .commit()
            .await
            .map_err(crate::error::mcp_internal)?;
        if let Some(authorization) = self.authorization {
            authorization
                .commit()
                .await
                .map_err(crate::error::mcp_internal)?;
        }
        Ok(())
    }

    pub(crate) async fn require_action(
        &mut self,
        action: crate::models::iam_policy::Action,
    ) -> Result<(), YorishiroError> {
        self.authorization = crate::models::access::authorize_workspace_action(
            &self.txn,
            &self.identity,
            &self.ctx,
            self.required,
            action,
        )
        .await?;
        Ok(())
    }

    pub(crate) fn auth_context(&self) -> &AuthContext {
        &self.ctx
    }

    pub(crate) fn workspace_id(&self) -> uuid::Uuid {
        self.workspace_id
    }
}

/// `authorize` splits its outcome into two kinds rather than a single failure case: a protocol-level failure (`Err`) and a scope-insufficient business outcome (`Ok` variant).
/// The former is a dead end an agent can't usefully retry (missing/invalid API key); the latter is information an agent can act on.
///
/// Handlers match on this directly rather than through a macro, so the `return` that ends the call on a denial is visible where it happens.
/// It cannot collapse into the `Err` side of the handler's own `Result`: `rmcp`'s `ToolRouter` fixes every tool's function type to `Result<CallToolResponse, ErrorData>` (`rmcp-3.4.0`, `handler/server/router/tool.rs:202`), and `ErrorData` is the protocol-level failure, which is not what a denial is.
pub(crate) enum AuthzOutcome {
    Authorized(Authorized),
    ScopeDenied(CallToolResult),
}

/// A connection-less version of `Authorized`: only authenticates and verifies scope, without opening a transaction.
/// Tools that do slow work before touching the database (embedding generation) use this instead and call `TenantDb::acquire_for_workspace` afterward.
pub(crate) enum VerifyOutcome {
    Verified(AuthContext),
    ScopeDenied(CallToolResult),
}

/// Copies the request's headers into the shape `auth::Authenticator` takes: the same thing the REST adapter does, so a replaced authenticator sees an MCP call exactly as it sees a REST one.
pub(super) fn extract_bearer_key(parts: &Parts) -> Result<&str, ErrorData> {
    auth::extract_bearer_key(parts).ok_or_else(|| {
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

/// The sole entry point for every tool handler.
/// Because there is no other way to obtain a `DatabaseTransaction`, forgetting the scope check is structurally impossible.
///
/// Shares `RequestAuth::authorize` with the REST adapter's `Authorized<R>` extractor; this just routes its result into the MCP protocol's two failure shapes (`ErrorData` at the protocol level, `CallToolResult` at the tool-result level).
pub(crate) async fn authorize(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<AuthzOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = auth::header_pairs(&parts.headers).map_err(header_error)?;

    let request_auth = auth::RequestAuth::new(ctx);
    let current = match request_auth.resolve(presented_key, &headers).await {
        Ok(current) => current,
        Err(error @ YorishiroError::ScopeInsufficient { .. }) => {
            return Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(error)));
        }
        Err(YorishiroError::Unauthenticated) => {
            return Err(ErrorData::invalid_request("authentication failed", None));
        }
        // A well-formed request with a value the credential cannot use (such as a malformed `x-workspace-id`) is the client's, not the server's, failure.
        Err(YorishiroError::ValidationFailed { message, .. }) => {
            return Err(ErrorData::invalid_params(message, None));
        }
        Err(error) => return Err(crate::error::mcp_internal(error)),
    };
    if let Err(error) = auth::require_scope(&current, required) {
        return Ok(AuthzOutcome::ScopeDenied(scope_denied(
            error,
            required,
            current.scope,
        )));
    }
    let workspace_id = match current.require_workspace() {
        Ok(workspace_id) => workspace_id,
        Err(error) => return Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(error))),
    };
    let identity = ctx.db.clone();
    match request_auth
        .begin(current, required >= ApiKeyScope::Write)
        .await
    {
        Ok((ctx, txn)) => Ok(AuthzOutcome::Authorized(Authorized {
            ctx,
            txn,
            identity,
            authorization: None,
            required,
            workspace_id,
        })),
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(err)))
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

/// Connection-less counterpart to `authorize`, used by tools that must run a slow operation (embedding generation) before touching the database.
/// See `RequestAuth::verify`.
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
