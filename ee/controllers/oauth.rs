//! `GET /auth/oauth/status|authorize|callback`: OAuth2/OIDC login, an additional way to obtain a Yorishiro API key alongside the community server's own `POST /auth/login`.
//!
//! All three routes carry the licence gate, so an unlicensed deployment answers `404` to every one of them before any of the below runs.
//! SSO login is a enterprise-edition feature, which is a decision about what the feature is; needing configuration on top of that does not make it a community route.
//!
//! They are also opt-in within a licensed deployment: `OAuthConfig::from_config` resolves to `Ok(None)` unless `YORISHIRO_OAUTH_ISSUER_URL` is set, in which case `authorize`/`callback` return `404 Not Found` before doing anything else, indistinguishable from the route simply not existing.
//! A set issuer with a missing `client_id`/`client_secret` is a different case, `Err`, and answers `500` naming the misconfiguration rather than either `404`.
//! `status` is the exception: past the gate it answers `200` whether or not OAuth is configured, and `500` on that same partial-configuration error, but never `404`.
//! That is what makes it the route `licence_gate.rs` tests the gate with, since its `404` can only come from the gate, while `authorize` and `callback` produce one of their own whenever the issuer is unset.

pub mod state_token;

use crate::YorishiroError;
use crate::controllers::ApiError;
use crate::error::ResultExt;
use crate::models::api_keys::IdentityApiKeys;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;

use crate::edition::ee::data::oauth::OAuthConfig;
use crate::edition::ee::dtos::oauth::{CallbackParams, OAuthStatus};
use crate::edition::ee::services::oauth;

/// Name of the CSRF cookie `authorize` sets and `callback` reads back.
const CSRF_COOKIE_NAME: &str = "ysr_oauth_csrf";
const INVITE_COOKIE_NAME: &str = "ysr_oauth_invite";

/// Starts a browser-bound OAuth flow for an existing account's invitation.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/auth/oauth/invites/accept", request_body = crate::edition::ee::dtos::oauth::AcceptOAuthInviteRequest, responses((status = 303, description = "Redirects to the identity provider using GET"), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(()), tag = "enterprise"))]
async fn accept_invite(
    State(ctx): State<AppContext>,
    Json(body): Json<crate::edition::ee::dtos::oauth::AcceptOAuthInviteRequest>,
) -> Result<Response, ApiError> {
    let config = OAuthConfig::from_config(&ctx.config)?.ok_or_else(not_found)?;
    if body.invite_token.len() != 48
        || !body
            .invite_token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(YorishiroError::ValidationFailed {
            message: "invalid invite token".into(),
            details: vec![],
            hint: "use the token from the invitation".into(),
        }
        .into());
    }
    let redirect = oauth::build_authorize_redirect(&config).await?;
    let payload = format!("{}.{}", redirect.csrf_cookie_value, body.invite_token);
    let signature = crate::edition::ee::controllers::hmac_sign::sign(
        &config.state_signing_key,
        payload.as_bytes(),
    );
    let secure = if config.cookies_require_secure() {
        "; Secure"
    } else {
        ""
    };
    let invite_cookie = format!(
        "{INVITE_COOKIE_NAME}={payload}.{signature}; HttpOnly; SameSite=Lax; Path=/auth/oauth/callback; Max-Age={}{}",
        state_token::STATE_TTL_SECS,
        secure
    );
    Ok((
        AppendHeaders([
            (
                header::SET_COOKIE,
                csrf_set_cookie(&redirect.csrf_cookie_value, config.cookies_require_secure()),
            ),
            (header::SET_COOKIE, invite_cookie),
        ]),
        Redirect::to(&redirect.url),
    )
        .into_response())
}

fn not_found() -> ApiError {
    YorishiroError::not_found("OAuth login is not configured on this deployment").into()
}

/// Reads `name`'s value out of the request's `Cookie` header, if present.
fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_string())
    })
}

/// Builds a `Set-Cookie` header value for the CSRF cookie: `HttpOnly`, `SameSite=Lax`, scoped to the callback path only, `Secure` when the configured redirect URI is itself `https://` (see `OAuthConfig::cookies_require_secure`), and expiring with the `state` it is bound to.
fn csrf_set_cookie(value: &str, secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    let max_age = state_token::STATE_TTL_SECS;
    format!(
        "{CSRF_COOKIE_NAME}={value}; HttpOnly; SameSite=Lax; Path=/auth/oauth/callback; \
         Max-Age={max_age}{secure_attr}",
    )
}

/// Clears the CSRF cookie: `Max-Age=0` on the same path it was set with, so the browser drops it immediately rather than leaving it to expire naturally.
fn csrf_clear_cookie() -> String {
    format!("{CSRF_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/auth/oauth/callback; Max-Age=0")
}

/// `GET /auth/oauth/status`: lets a client decide whether to show the "Sign in with SSO" button, without hardcoding a build-time assumption about whether OAuth is configured.
/// Unlike the other two routes, this one never answers `404` (it reports `enabled: false` when unconfigured, and only the partial-configuration `500` above departs from `200`), since a client that could not tell "not configured" apart from "not present" would have no way to decide whether to show the button at all.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/auth/oauth/status", responses((status = 200, body = crate::edition::ee::controllers::openapi::OAuthStatus), (status = 500, body = crate::controllers::openapi::ApiErrorBody)), security(()), tag = "enterprise"))]
async fn status(State(ctx): State<AppContext>) -> Result<Json<OAuthStatus>, ApiError> {
    Ok(Json(OAuthStatus {
        enabled: OAuthConfig::from_config(&ctx.config)?.is_some(),
    }))
}

/// `GET /auth/oauth/authorize`: starts the login flow by redirecting the browser to the identity provider's own authorization endpoint.
/// Also sets the CSRF cookie that `callback` checks the returning `state` against: without it, `state`'s HMAC signature alone only proves this server issued *some* state, not that the browser presenting it at callback time is the one that started this flow.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/auth/oauth/authorize", responses((status = 302, description = "Redirects to the configured identity provider"), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 500, body = crate::controllers::openapi::ApiErrorBody)), security(()), tag = "enterprise"))]
async fn authorize(State(ctx): State<AppContext>) -> Result<Response, ApiError> {
    let config = OAuthConfig::from_config(&ctx.config)?.ok_or_else(not_found)?;

    let redirect = oauth::build_authorize_redirect(&config)
        .await
        .inspect_err(|err| tracing::error!(error = %err, "failed to build OAuth authorize URL"))?;

    let set_cookie = csrf_set_cookie(&redirect.csrf_cookie_value, config.cookies_require_secure());

    Ok((
        AppendHeaders([
            (header::SET_COOKIE, set_cookie),
            (header::SET_COOKIE, clear_invite_cookie()),
        ]),
        Redirect::to(&redirect.url),
    )
        .into_response())
}

/// `GET /auth/oauth/callback`: the identity provider's redirect target.
/// Exchanges the authorization code for tokens, verifies the ID token, resolves (or auto-provisions) the Yorishiro user/tenant/workspace it corresponds to, issues an API key exactly the way `POST /auth/login` does, and hands it to the client via a URL fragment (`#api_key=...`), a fragment rather than a query parameter so the key never appears in server access logs or gets sent back to any server in a `Referer` header.
#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/auth/oauth/callback", params(("code" = Option<String>, Query), ("state" = Option<String>, Query), ("error" = Option<String>, Query)), responses((status = 302, description = "Redirects to the application login result"), (status = 400, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 500, body = crate::controllers::openapi::ApiErrorBody)), security(()), tag = "enterprise"))]
async fn callback(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Result<Response, ApiError> {
    let config = OAuthConfig::from_config(&ctx.config)?.ok_or_else(not_found)?;

    let csrf_cookie_value = read_cookie(&headers, CSRF_COOKIE_NAME);
    let clear_cookie = csrf_clear_cookie();

    if params.error.is_some() {
        tracing::warn!("identity provider returned an error on the OAuth callback");
        return Ok(login_failure_redirect(clear_cookie));
    }

    let (Some(code), Some(request_state)) = (params.code, params.state) else {
        return Ok(login_failure_redirect(clear_cookie));
    };

    let identity =
        oauth::handle_callback(&config, &code, &request_state, csrf_cookie_value.as_deref())
            .await?;

    if let Some(cookie) = read_cookie(&headers, INVITE_COOKIE_NAME) {
        let (payload, signature) = cookie
            .rsplit_once('.')
            .ok_or(YorishiroError::Unauthenticated)?;
        let (csrf, token) = payload
            .split_once('.')
            .ok_or(YorishiroError::Unauthenticated)?;
        if csrf_cookie_value.as_deref() != Some(csrf)
            || !crate::edition::ee::controllers::hmac_sign::verify(
                &config.state_signing_key,
                payload.as_bytes(),
                signature,
            )
        {
            return Err(YorishiroError::Unauthenticated.into());
        }
        crate::edition::ee::models::user_users::accept_invite(&ctx.db, &identity, token).await?;
        return Ok((
            StatusCode::FOUND,
            AppendHeaders([
                (header::LOCATION, "/#/login?invite=accepted".to_owned()),
                (header::SET_COOKIE, clear_cookie),
                (header::SET_COOKIE, clear_invite_cookie()),
            ]),
        )
            .into_response());
    }

    let max_tenants = ctx
        .shared_store
        .get::<crate::data::tenant_policy::TenantCap>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(anyhow::anyhow!(
                "tenant cap policy missing"
            )))
        })?
        .resolve();

    let txn = ctx.db.begin().await.internal()?;
    let provisioned = crate::edition::ee::models::user_users::find_or_create(
        &txn,
        "oidc",
        &identity.subject_id,
        identity.email.as_deref(),
        identity.display_name.as_deref(),
        max_tenants,
        *ctx.shared_store
            .get::<std::sync::Arc<bool>>()
            .ok_or_else(|| {
                ApiError(YorishiroError::Internal(anyhow::anyhow!(
                    "plan policy missing"
                )))
            })?,
    )
    .await?;
    txn.commit().await.internal()?;

    let created = IdentityApiKeys::create_api_key(
        &ctx.db,
        provisioned.workspace_id,
        provisioned.scope,
        Some(provisioned.user_id),
        false,
    )
    .await?;

    Ok(login_success_redirect(&created.plaintext, clear_cookie))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_status),
        crate::controllers::route_inventory::path_doc(__path_authorize),
        crate::controllers::route_inventory::path_doc(__path_callback),
        crate::controllers::route_inventory::path_doc(__path_accept_invite),
    ]
}

fn login_success_redirect(api_key: &str, clear_cookie: String) -> Response {
    // Generated keys contain only alphanumeric characters and underscores, so no fragment encoding is needed.
    let location = format!("/#api_key={api_key}");
    (
        StatusCode::FOUND,
        AppendHeaders([
            (header::LOCATION, location),
            (header::SET_COOKIE, clear_cookie),
            (header::SET_COOKIE, clear_invite_cookie()),
        ]),
    )
        .into_response()
}

fn login_failure_redirect(clear_cookie: String) -> Response {
    let location = "/#/login?error=oauth_failed".to_string();
    (
        StatusCode::FOUND,
        AppendHeaders([
            (header::LOCATION, location),
            (header::SET_COOKIE, clear_cookie),
            (header::SET_COOKIE, clear_invite_cookie()),
        ]),
    )
        .into_response()
}

fn clear_invite_cookie() -> String {
    format!("{INVITE_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/auth/oauth/callback; Max-Age=0")
}

pub fn routes() -> Routes {
    use axum::routing::get;
    use axum::routing::post;

    Routes::new()
        .prefix("auth/oauth")
        .add("/status", get(status))
        .add("/authorize", get(authorize))
        .add("/callback", get(callback))
        .add("/invites/accept", post(accept_invite))
}
