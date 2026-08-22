mod entities;
mod import;
mod recall;
mod relations;
mod schemas;
mod search;
mod template_library;

use axum::http::request::Parts;
use loco_rs::app::AppContext;
use rmcp::ErrorData;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool_handler};
use sea_orm::DatabaseTransaction;

use crate::controllers::extractors::{authenticator, db_handle};
use crate::error::YorishiroError;
use crate::services::auth::{self, ApiKeyScope, AuthContext};

/// Yorishiro MCP server, assembled from each domain's `#[tool_router]` implementation.
///
/// The sixth seam `ee/` composes against (`specs` CLAUDE.md): `ee/` calls this type directly,
/// so a grep finds the caller and it's deliberately not on the five-contract immunity list.
#[derive(Clone)]
pub struct YorishiroMcpServer {
    ctx: AppContext,
    tool_router: ToolRouter<Self>,
}

impl YorishiroMcpServer {
    pub fn new(ctx: AppContext) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router_entities()
                + Self::tool_router_import()
                + Self::tool_router_recall()
                + Self::tool_router_relations()
                + Self::tool_router_schemas()
                + Self::tool_router_search()
                + Self::tool_router_template_library(),
        }
    }
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for YorishiroMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Yorishiro is a multi-tenant knowledge store with user-defined schemas. \
             Every tool call requires authentication via an `Authorization: Bearer <api-key>` \
             header, and the tools available depend on the API key's scope \
             (read/write/schema, where higher scopes include the permissions of lower ones).",
        )
    }
}

/// Auth context plus a transaction with RLS already configured, held by calls that passed
/// authentication and scope checks.
pub(super) struct Authorized {
    pub(super) ctx: AuthContext,
    txn: DatabaseTransaction,
}

impl Authorized {
    pub(super) fn txn(&self) -> &DatabaseTransaction {
        &self.txn
    }

    /// Commits the transaction. Every write handler must call this before returning `Ok`.
    pub(super) async fn commit(self) -> Result<(), ErrorData> {
        self.txn
            .commit()
            .await
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))
    }
}

/// `authorize` splits its outcome into two kinds rather than a single failure case: a
/// protocol-level failure (`Err`) and a scope-insufficient business outcome (`Ok` variant). The
/// former is a dead end an agent can't usefully retry (missing/invalid API key); the latter is
/// information an agent can act on.
pub(super) enum AuthzOutcome {
    Authorized(Authorized),
    ScopeDenied(CallToolResult),
}

/// A connection-less version of `Authorized`: only authenticates and verifies scope, without
/// opening a transaction. Tools that do slow work before touching the database (embedding
/// generation) use this instead and call `TenantDb::acquire_for_workspace` afterward.
pub(super) enum VerifyOutcome {
    Verified(AuthContext),
    ScopeDenied(CallToolResult),
}

/// Copies the request's headers into the shape `auth::Authenticator` takes: the same thing the
/// REST adapter does, so a replaced authenticator sees an MCP call exactly as it sees a REST one.
fn header_pairs(parts: &Parts) -> Vec<(String, String)> {
    parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect()
}

fn extract_bearer_key(parts: &Parts) -> Result<&str, ErrorData> {
    auth::bearer_credential(
        parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    )
    .ok_or_else(|| ErrorData::invalid_request("missing or malformed Authorization header", None))
}

/// The sole entry point for every tool handler. Because there is no other way to obtain a
/// `DatabaseTransaction`, forgetting the scope check is structurally impossible.
///
/// Shares `services::auth::authorize` with the REST adapter's `Authorized<R>` extractor; this
/// just routes its result into the MCP protocol's two failure shapes (`ErrorData` at the
/// protocol level, `CallToolResult` at the tool-result level) since one MCP route dispatches
/// many tools with different required scopes, which a route-level, compile-time-typed extractor
/// can't express.
pub(super) async fn authorize(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<AuthzOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = header_pairs(parts);

    let db = db_handle(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;
    let auth_impl =
        authenticator(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;

    match auth::authorize(&db, auth_impl.as_ref(), presented_key, required, &headers).await {
        Ok((ctx, txn)) => Ok(AuthzOutcome::Authorized(Authorized { ctx, txn })),
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
    }
}

/// Connection-less counterpart to `authorize`, used by tools that must run a slow step (embedding
/// generation) before touching the database. See `services::auth::authorize_scope`.
pub(super) async fn verify(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<VerifyOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = header_pairs(parts);

    let db = db_handle(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;
    let auth_impl =
        authenticator(ctx).map_err(|err| ErrorData::internal_error(err.0.to_string(), None))?;

    match auth::authorize_scope(&db, auth_impl.as_ref(), presented_key, required, &headers).await {
        Ok(ctx) => Ok(VerifyOutcome::Verified(ctx)),
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(VerifyOutcome::ScopeDenied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
    }
}

/// Converts a business-logic error into a tool call result (`is_error: true`). `Internal` errors
/// are logged with detail but only a generic message reaches the client, matching the REST
/// adapter's `ApiError` policy.
pub(super) fn err_to_tool_result(err: YorishiroError) -> CallToolResult {
    let message = match err {
        YorishiroError::Internal(err) => {
            tracing::error!(error = %err, "internal error in mcp tool handler");
            "internal server error".to_string()
        }
        YorishiroError::ValidationFailed {
            message,
            details,
            hint,
        } => {
            let mut msg = message;
            if !details.is_empty() {
                let detail_lines: Vec<String> = details
                    .iter()
                    .map(|d| format!("{}: {}", d.field, d.problem))
                    .collect();
                msg = format!("{msg}\n  {}", detail_lines.join("\n  "));
            }
            if !hint.is_empty() {
                msg = format!("{msg}\nhint: {hint}");
            }
            msg
        }
        other => other.to_string(),
    };
    CallToolResult::error(vec![ContentBlock::text(message)])
}

pub(super) fn ok_json(value: impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value)
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Authenticates the caller and verifies scope, opening an RLS-scoped transaction. Expands to
/// the `Authorized` value on success; on a scope-denied outcome it early-returns the tool result.
/// A macro rather than a function because it early-returns from the enclosing handler (which
/// must return `Result<CallToolResult, ErrorData>`).
macro_rules! authorized {
    ($ctx:expr, $parts:expr, $scope:expr) => {
        match $crate::services::mcp::authorize($ctx, $parts, $scope).await? {
            $crate::services::mcp::AuthzOutcome::Authorized(authorized) => authorized,
            $crate::services::mcp::AuthzOutcome::ScopeDenied(result) => {
                return ::core::result::Result::Ok(result);
            }
        }
    };
}
pub(crate) use authorized;

/// Connection-less counterpart to `authorized!`. Expands to the caller's `AuthContext` on
/// success; on a scope-denied outcome it early-returns the tool result.
macro_rules! verified {
    ($ctx:expr, $parts:expr, $scope:expr) => {
        match $crate::services::mcp::verify($ctx, $parts, $scope).await? {
            $crate::services::mcp::VerifyOutcome::Verified(ctx) => ctx,
            $crate::services::mcp::VerifyOutcome::ScopeDenied(result) => {
                return ::core::result::Result::Ok(result);
            }
        }
    };
}
pub(crate) use verified;

/// Unwraps a repository/service call's `Ok` value, or early-returns the tool-level error result
/// for its `Err` value. A macro rather than a function because it early-returns from the
/// enclosing handler. Only fits call sites whose `Err` arm is exactly
/// `Ok(err_to_tool_result(err))`.
macro_rules! mcp_try {
    ($expr:expr) => {
        match $expr {
            ::core::result::Result::Ok(val) => val,
            ::core::result::Result::Err(err) => {
                return ::core::result::Result::Ok($crate::services::mcp::err_to_tool_result(err));
            }
        }
    };
}
pub(crate) use mcp_try;
