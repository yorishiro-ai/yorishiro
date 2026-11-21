//! Authenticating a request's bearer key and authorizing each tool call against its scope and the active policy.

use super::{
    ApiKeyScope, AppContext, AuthContext, CallToolResult, DatabaseTransaction, ErrorData, Parts,
    YorishiroError, auth, err_to_tool_result,
};

/// Auth context plus a transaction with RLS already configured, held by calls that passed authentication and scope checks.
pub(crate) struct Authorized {
    ctx: AuthContext,
    txn: DatabaseTransaction,
    identity: sea_orm::DatabaseConnection,
    authorization: Option<DatabaseTransaction>,
    inferred_resource: bool,
    required: ApiKeyScope,
    workspace_id: uuid::Uuid,
}

impl Authorized {
    pub(crate) async fn for_target(
        app: &AppContext,
        actor: &AuthContext,
        required: ApiKeyScope,
        target: crate::models::iam::access::WorkspaceTarget,
    ) -> Result<Self, YorishiroError> {
        auth::require_scope(actor, required)?;
        let inferred_resource = target.inferred_resource(actor);
        let mut actor = actor.clone();
        let workspace_id = target
            .resolve(&app.db, &actor, required)
            .await
            .map_err(|error| {
                crate::models::iam::access::hide_inferred_resource(inferred_resource, error)
            })?;
        actor.workspace_id = Some(workspace_id);
        let (ctx, txn) = auth::RequestAuth::new(app)
            .begin(actor, required >= ApiKeyScope::Write)
            .await?;
        Ok(Self {
            ctx,
            txn,
            identity: app.db.clone(),
            authorization: None,
            inferred_resource,
            required,
            workspace_id,
        })
    }
    pub(crate) const fn txn(&self) -> &DatabaseTransaction {
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
        action: crate::models::iam::policy::Action,
    ) -> Result<(), YorishiroError> {
        self.authorization = crate::models::iam::access::authorize_workspace_action(
            &self.txn,
            &self.identity,
            &self.ctx,
            self.required,
            action,
        )
        .await
        .map_err(|error| {
            crate::models::iam::access::hide_inferred_resource(self.inferred_resource, error)
        })?;
        Ok(())
    }

    pub(crate) const fn auth_context(&self) -> &AuthContext {
        &self.ctx
    }

    pub(crate) const fn workspace_id(&self) -> uuid::Uuid {
        self.workspace_id
    }
}

/// A connection-less version of `Authorized`: only authenticates and verifies scope, without opening a transaction.
/// Tools that do slow work before touching the database (embedding generation) use this instead and call `TenantDb::acquire_for_workspace` afterward.
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
