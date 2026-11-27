use std::{cell::RefCell, time::Instant};

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use loco_rs::{
    app::AppContext,
    controller::middleware::request_id::{LocoRequestId, request_id_middleware},
};
use tracing::Instrument;

use crate::models::access_keys::AuthContext;
use crate::models::iam::policy::{Action, Resource};

#[derive(Default)]
struct Operation {
    caller: Option<AuthContext>,
    permissions: Vec<(Action, Resource)>,
    denied: bool,
}

tokio::task_local! {
    static OPERATION: RefCell<Operation>;
}

pub(crate) fn authenticated(actor: &AuthContext) {
    let _ = OPERATION.try_with(|operation| operation.borrow_mut().caller = Some(actor.clone()));
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

/// What identifies one call before it is authenticated, as an audit trail records it: where it came from and whether it can change anything.
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

pub(crate) async fn track<T>(
    call: Call<'_>,
    run: impl Future<Output = T>,
    classify: impl FnOnce(&T) -> (&'static str, Option<i64>),
) -> T {
    let Call {
        transport,
        operation,
        request_id,
        read_only,
        source_ip,
        user_agent,
    } = call;
    let request_id = request_id.map_or("req-id-none", LocoRequestId::get);
    let span = tracing::error_span!("request", transport, operation, request_id);
    let started = Instant::now();
    OPERATION.scope(RefCell::default(), async move {
        let response = run.await;
        let (outcome, code) = classify(&response);
        let record = OPERATION.with(RefCell::take);
        let caller = record.caller.as_ref();
        let access_key = caller.map(AuthContext::yrn);
        let permissions = record.permissions.into_iter()
            .map(|(action, target)| format!("{} {}", action.qualified(), target.yrn()))
            .collect::<Vec<_>>();
        tracing::info!(
            transport,
            operation,
            request_id,
            read_only,
            source_ip = source_ip.map(tracing::field::display),
            user_agent,
            caller = access_key.as_deref().unwrap_or("unauthenticated"),
            tenant_id = caller.map(|actor| tracing::field::display(actor.tenant_id)),
            user_id = caller.and_then(|actor| actor.user_id).map(tracing::field::display),
            workspace = caller.and_then(AuthContext::workspace_yrn).as_deref(),
            outcome = if record.denied && outcome != "success" { "denied" } else { outcome },
            code,
            permissions = ?permissions,
            latency_ms = started.elapsed().as_secs_f64() * 1000.0,
            "request completed",
        );
        response
    }.instrument(span)).await
}

/// The request target used by the access log.
/// Query strings are deliberately excluded because callback parameters can carry codes, state, and provider error details.
pub fn path_only(uri: &axum::http::Uri) -> &str {
    uri.path()
}

pub(crate) fn layer(mut router: axum::Router, ctx: &AppContext) -> axum::Router {
    let config = &ctx.config.server.middlewares;
    if config.logger.as_ref().is_none_or(|logger| logger.enable) {
        router = router.layer(axum::middleware::from_fn(log_request));
    }
    if config.request_id.as_ref().is_none_or(|id| id.enable) {
        router = router.layer(axum::middleware::from_fn(request_id_middleware));
    }
    router
}

async fn log_request(request: Request, next: Next) -> Response {
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
        read_only: matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS"),
        source_ip: source_ip(request.extensions()),
        user_agent: &user_agent,
    };
    track(call, next.run(request), |response| {
        let status = response.status();
        let outcome = if is_denial(status) {
            "denied"
        } else if status.is_client_error() || status.is_server_error() {
            "failed"
        } else {
            "success"
        };
        (outcome, Some(i64::from(status.as_u16())))
    })
    .await
}

pub(crate) const fn is_denial(status: axum::http::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 423 | 429)
}
