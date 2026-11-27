use std::{cell::RefCell, time::Instant};

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};
use loco_rs::{
    app::AppContext,
    controller::middleware::request_id::{LocoRequestId, request_id_middleware},
};
use tracing::Instrument;

use sea_orm::DatabaseConnection;

use crate::models::access_keys::AuthContext;
use crate::models::api_events;
use crate::models::iam::policy::{Action, Resource};

#[derive(Default)]
struct Operation {
    /// Where the `authenticated` row of a write goes, until it has been written.
    pending: Option<(DatabaseConnection, api_events::Call)>,
    request_id: String,
    caller: Option<AuthContext>,
    permissions: Vec<(Action, Resource)>,
    denied: bool,
}

tokio::task_local! {
    static OPERATION: RefCell<Operation>;
}

/// Notes who the call is, and for a write records it before the call goes on.
///
/// # Errors
/// Returns an error when the write cannot be attributed in the trail, so the call is refused.
pub(crate) async fn authenticated(actor: &AuthContext) -> Result<(), crate::YorishiroError> {
    let pending = OPERATION
        .try_with(|operation| {
            let mut operation = operation.borrow_mut();
            operation.caller = Some(actor.clone());
            operation.pending.take()
        })
        .ok()
        .flatten();
    if let Some((db, event)) = pending {
        event.authenticated(&db, actor).await?;
    }
    Ok(())
}

/// The request the running code is serving, so work it starts can be traced back to it.
pub(crate) fn current_request_id() -> Option<String> {
    OPERATION
        .try_with(|operation| operation.borrow().request_id.clone())
        .ok()
        .filter(|id| !id.is_empty())
}

pub(crate) fn authorized(action: Action, target: Resource) {
    let _ = OPERATION.try_with(|operation| {
        let mut operation = operation.borrow_mut();
        if !operation.permissions.contains(&(action, target)) {
            operation.permissions.push((action, target));
        }
    });
}

pub(crate) fn denied() {
    let _ = OPERATION.try_with(|operation| operation.borrow_mut().denied = true);
}

/// What identifies one call before it is authenticated: where it came from and whether it can change anything.
pub(crate) struct Call<'a> {
    pub transport: &'static str,
    pub operation: &'a str,
    pub request_id: Option<&'a LocoRequestId>,
    pub read_only: bool,
    pub source_ip: Option<std::net::IpAddr>,
    pub user_agent: &'a str,
}

pub(crate) fn source_ip(extensions: &axum::http::Extensions) -> Option<std::net::IpAddr> {
    extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|axum::extract::ConnectInfo(address)| address.ip())
}

pub(crate) fn user_agent(headers: &axum::http::HeaderMap) -> &str {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

/// Runs one call and records it in the durable trail and in the log.
///
/// A call that can change something is refused, before it runs, when its start cannot be recorded.
/// A read is refused, before its data is returned, when its completion cannot be recorded.
/// A write that already took effect keeps its response when only its completion is lost: the `started` row marks it and the full record goes to the error log.
pub(crate) async fn track<T>(
    call: Call<'_>,
    db: &DatabaseConnection,
    run: impl Future<Output = T>,
    classify: impl FnOnce(&T) -> (&'static str, Option<i64>),
    unavailable: impl FnOnce() -> T,
) -> T {
    let request_id = call.request_id.map_or("req-id-none", LocoRequestId::get);
    let event = api_events::Call {
        request_id: request_id.to_owned(),
        transport: call.transport,
        event_name: call.operation.to_owned(),
        read_only: call.read_only,
        source_ip: call.source_ip.map(|address| address.to_string()),
        user_agent: call.user_agent.to_owned(),
    };
    let span = tracing::error_span!(
        "request",
        transport = call.transport,
        operation = call.operation,
        request_id
    );
    let started = Instant::now();
    let operation = Operation {
        request_id: request_id.to_owned(),
        pending: (!event.read_only).then(|| (db.clone(), event.clone())),
        ..Operation::default()
    };
    OPERATION
        .scope(
            RefCell::new(operation),
            async move {
                if !event.read_only
                    && let Err(error) = event.start(db).await
                {
                    tracing::error!(%error, "refused: the call could not be recorded");
                    return unavailable();
                }
                let response = run.await;
                let (outcome, code) = classify(&response);
                let record = OPERATION.with(RefCell::take);
                let permissions: Vec<String> = record
                    .permissions
                    .iter()
                    .map(|(action, target)| format!("{} {}", action.qualified(), target.yrn()))
                    .collect();
                let completion = api_events::Completion {
                    permissions,
                    outcome: if record.denied && outcome != "success" {
                        "denied"
                    } else {
                        outcome
                    },
                    status_code: code,
                    latency_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
                    caller: record.caller,
                };
                let caller = completion.caller.as_ref();
                tracing::info!(
                    transport = event.transport,
                    operation = event.event_name,
                    request_id,
                    read_only = event.read_only,
                    source_ip = event.source_ip,
                    user_agent = event.user_agent,
                    caller = caller
                        .map(AuthContext::yrn)
                        .as_deref()
                        .unwrap_or("unauthenticated"),
                    tenant_id = caller.map(|actor| tracing::field::display(actor.tenant_id)),
                    user_id = caller
                        .and_then(|actor| actor.user_id)
                        .map(tracing::field::display),
                    workspace = caller.and_then(AuthContext::workspace_yrn).as_deref(),
                    outcome = completion.outcome,
                    code,
                    permissions = ?completion.permissions,
                    latency_ms = completion.latency_ms,
                    "request completed",
                );
                if let Err(error) = event.finish(db, &completion).await {
                    tracing::error!(%error, "the completed call could not be recorded");
                    if event.read_only {
                        return unavailable();
                    }
                }
                response
            }
            .instrument(span),
        )
        .await
}

/// The request target used by the access log.
/// Query strings are deliberately excluded because callback parameters can carry codes, state, and provider error details.
pub fn path_only(uri: &axum::http::Uri) -> &str {
    uri.path()
}

pub(crate) fn layer(mut router: axum::Router, ctx: &AppContext) -> axum::Router {
    let config = &ctx.config.server.middlewares;
    if config.logger.as_ref().is_none_or(|logger| logger.enable) {
        router = router.layer(axum::middleware::from_fn_with_state(
            ctx.db.clone(),
            log_request,
        ));
    }
    if config.request_id.as_ref().is_none_or(|id| id.enable) {
        router = router.layer(axum::middleware::from_fn(request_id_middleware));
    }
    router
}

async fn log_request(
    axum::extract::State(db): axum::extract::State<DatabaseConnection>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == axum::http::Method::OPTIONS
        || matches!(request.uri().path(), "/_ping" | "/_health" | "/_readiness")
    {
        return next.run(request).await;
    }
    let is_mcp = path_only(request.uri()) == "/mcp";
    let route = request.extensions().get::<MatchedPath>().map_or(
        if is_mcp { "/mcp" } else { "<unmatched>" },
        MatchedPath::as_str,
    );
    let operation = format!("{} {route}", request.method());
    let request_id = request.extensions().get::<LocoRequestId>().cloned();
    let user_agent = user_agent(request.headers()).to_owned();
    let call = Call {
        transport: if is_mcp { "mcp_http" } else { "rest" },
        operation: &operation,
        request_id: request_id.as_ref(),
        read_only: is_mcp || matches!(request.method().as_str(), "GET" | "HEAD"),
        source_ip: source_ip(request.extensions()),
        user_agent: &user_agent,
    };
    track(
        call,
        &db,
        next.run(request),
        |response: &Response| {
            let status = response.status();
            let outcome = if is_denial(status) {
                "denied"
            } else if status.is_client_error() || status.is_server_error() {
                "failed"
            } else {
                "success"
            };
            (outcome, Some(i64::from(status.as_u16())))
        },
        || {
            crate::controllers::ApiError::from(crate::YorishiroError::BackendUnavailable {
                message: "the call could not be recorded in the audit trail".into(),
            })
            .into_response()
        },
    )
    .await
}

pub(crate) const fn is_denial(status: axum::http::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 423 | 429)
}
