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
use crate::db::AppContextBackend;
use crate::error::YorishiroError;
use crate::services::auth::{self, ApiKeyScope, AuthContext};

/// Yorishiro MCP server, assembled from each edition's `#[tool_router]` implementations.
#[derive(Clone)]
pub struct YorishiroMcpServer {
    ctx: AppContext,
    tool_router: ToolRouter<Self>,
}

impl YorishiroMcpServer {
    pub(crate) fn new(ctx: AppContext, tool_router: ToolRouter<Self>) -> Self {
        Self { ctx, tool_router }
    }

    pub(crate) fn app_context(&self) -> &AppContext {
        &self.ctx
    }
}

/// The complete community-edition tool set.
pub(crate) fn community_tool_router() -> ToolRouter<YorishiroMcpServer> {
    YorishiroMcpServer::tool_router_entities()
        + YorishiroMcpServer::tool_router_import()
        + YorishiroMcpServer::tool_router_recall()
        + YorishiroMcpServer::tool_router_relations()
        + YorishiroMcpServer::tool_router_schemas()
        + YorishiroMcpServer::tool_router_search()
        + YorishiroMcpServer::tool_router_template_library()
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

/// Auth context plus a transaction with RLS already configured, held by calls that passed authentication and scope checks.
pub(crate) struct Authorized {
    ctx: AuthContext,
    txn: DatabaseTransaction,
}

impl Authorized {
    pub(super) fn txn(&self) -> &DatabaseTransaction {
        &self.txn
    }

    /// Commits the transaction.
    /// Every write handler must call this before returning `Ok`.
    pub(super) async fn commit(self) -> Result<(), ErrorData> {
        self.txn
            .commit()
            .await
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))
    }

    pub(crate) fn auth_context(&self) -> &AuthContext {
        &self.ctx
    }
}

/// `authorize` splits its outcome into two kinds rather than a single failure case: a protocol-level failure (`Err`) and a scope-insufficient business outcome (`Ok` variant).
/// The former is a dead end an agent can't usefully retry (missing/invalid API key); the latter is information an agent can act on.
///
/// Handlers match on this directly rather than through a macro, so the `return` that ends the call on a denial is visible where it happens.
/// It cannot collapse into the `Err` side of the handler's own `Result`: `rmcp`'s `ToolRouter` fixes every tool's function type to `Result<CallToolResponse, ErrorData>` (`rmcp-3.0.1`, `handler/server/router/tool.rs:202`), and `ErrorData` is the protocol-level failure, which is not what a denial is.
pub(crate) enum AuthzOutcome {
    Authorized(Authorized),
    ScopeDenied(CallToolResult),
}

/// A connection-less version of `Authorized`: only authenticates and verifies scope, without opening a transaction.
/// Tools that do slow work before touching the database (embedding generation) use this instead and call `TenantDb::acquire_for_workspace` afterward.
pub(super) enum VerifyOutcome {
    Verified(AuthContext),
    ScopeDenied(CallToolResult),
}

/// Copies the request's headers into the shape `auth::Authenticator` takes: the same thing the REST adapter does, so a replaced authenticator sees an MCP call exactly as it sees a REST one.
fn extract_bearer_key(parts: &Parts) -> Result<&str, ErrorData> {
    auth::extract_bearer_key(parts).ok_or_else(|| {
        ErrorData::invalid_request("missing or malformed Authorization header", None)
    })
}

/// The sole entry point for every tool handler.
/// Because there is no other way to obtain a `DatabaseTransaction`, forgetting the scope check is structurally impossible.
///
/// Shares `services::auth::authorize` with the REST adapter's `Authorized<R>` extractor; this just routes its result into the MCP protocol's two failure shapes (`ErrorData` at the protocol level, `CallToolResult` at the tool-result level).
pub(crate) async fn authorize(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<AuthzOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = auth::header_pairs(parts);

    // No DbHandle/Authenticator is built for SQLite (Hooks::after_context): this path authenticates
    // directly against ctx.db and opens a plain transaction, mirroring the REST adapter's
    // `Authorized<R>` SQLite extractor.
    if ctx.is_sqlite() {
        return match auth::authorize_sqlite(&ctx.db, presented_key, required).await {
            Ok((ctx, txn)) => Ok(AuthzOutcome::Authorized(Authorized { ctx, txn })),
            Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
                Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(err)))
            }
            Err(YorishiroError::Unauthenticated) => {
                Err(ErrorData::invalid_request("authentication failed", None))
            }
            Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
        };
    }

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

/// Connection-less counterpart to `authorize`, used by tools that must run a slow operation (embedding generation) before touching the database.
/// See `services::auth::authorize_scope`.
pub(super) async fn verify(
    ctx: &AppContext,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<VerifyOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;
    let headers = auth::header_pairs(parts);

    // No DbHandle/Authenticator is built for SQLite (Hooks::after_context): this path authenticates
    // directly against ctx.db instead of going through the Authenticator trait, mirroring the REST
    // adapter's `AuthContext` extractor (extractors.rs) and `Verified<R>` extractor.
    if ctx.is_sqlite() {
        return match auth::authorize_scope_sqlite(&ctx.db, presented_key, required).await {
            Ok(ctx) => Ok(VerifyOutcome::Verified(ctx)),
            Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
                Ok(VerifyOutcome::ScopeDenied(err_to_tool_result(err)))
            }
            Err(YorishiroError::Unauthenticated) => {
                Err(ErrorData::invalid_request("authentication failed", None))
            }
            Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
        };
    }

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

/// Converts a business-logic error into a tool call result (`is_error: true`).
/// `Internal` errors are logged with detail but only a generic message reaches the client, matching the REST adapter's `ApiError` policy.
pub(crate) fn err_to_tool_result(err: YorishiroError) -> CallToolResult {
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

pub(crate) fn ok_json(value: impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value)
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

#[cfg(test)]
mod tests {
    use super::community_tool_router;

    #[test]
    fn community_tool_inventory_is_exact() {
        let names: Vec<_> = community_tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(
            names,
            [
                "create_entity",
                "create_relation",
                "create_schema",
                "delete_entity",
                "delete_relation",
                "fill_defaults",
                "get_active_schema",
                "get_entity",
                "get_entity_drift",
                "get_entity_type_json_schema",
                "get_relation",
                "get_schema_by_id",
                "get_template_library_item",
                "import_jsonl",
                "list_entities",
                "list_relations",
                "list_schemas",
                "list_template_library",
                "list_templates",
                "migration_dry_run",
                "recall_context",
                "search_entities",
                "set_relation_status",
                "update_entity",
            ]
            .map(str::to_owned)
            .to_vec()
        );
    }
}
