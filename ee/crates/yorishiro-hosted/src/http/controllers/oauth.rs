use axum::Json;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use cookie::time::Duration;
use serde::{Deserialize, Serialize};
use yorishiro_core::services::auth;
use yorishiro_core::{ResultExt, YorishiroError};

use crate::error::HostedApiError;
use crate::services::oauth;
use crate::state::HostedState;

/// Name of the CSRF cookie `authorize` sets and `callback` reads back: see `state_token` module docs for why this binding exists.
const CSRF_COOKIE_NAME: &str = "ysr_oauth_csrf";

/// The two `/auth/oauth/authorize|callback` routes below are enterprise-only *and* opt-in within the enterprise binary: `oauth_config` is `None` unless `YORISHIRO_OAUTH_ISSUER_URL` is set, in which case both return `404 Not Found` before doing anything else, indistinguishable from the route simply not existing, which is exactly the community-edition behavior this preserves when OAuth isn't configured.
fn not_found() -> HostedApiError {
    YorishiroError::not_found("OAuth login is not configured on this deployment").into()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OAuthStatus {
    /// `false` when `YORISHIRO_OAUTH_ISSUER_URL` is unset, in which case the other two `/auth/oauth/*` routes return `404`.
    pub enabled: bool,
}

/// `GET /auth/oauth/status`: lets the Web UI's login page decide whether to show the "Sign in with SSO" button, without hardcoding a build-time assumption about whether OAuth is configured.
/// Unlike the other two routes, this one always returns `200` (`enabled: false` when unconfigured) rather than `404`, since a client that can't tell "not configured" apart from "not present" would have no way to decide whether to show the button at all.
#[utoipa::path(
    get,
    path = "/auth/oauth/status",
    responses(
        (status = 200, description = "Whether OAuth login is configured on this deployment. Always answers 200, and is deliberately not rate-limited: it returns no secret and the Web UI's login page calls it on every load", body = OAuthStatus),
    ),
    tag = "hosted-oauth",
)]
pub async fn status(State(state): State<HostedState>) -> Json<OAuthStatus> {
    Json(OAuthStatus {
        enabled: state.oauth_config.is_some(),
    })
}

/// `GET /auth/oauth/authorize`: starts the login flow by redirecting the browser to the identity provider's own authorization endpoint (discovered from `YORISHIRO_OAUTH_ISSUER_URL`).
/// Also sets the CSRF cookie (see `state_token` module docs) that `callback` will check the returning `state` against: without it, `state`'s HMAC signature alone only proves this server issued *some* state, not that the browser presenting it at callback time is the one that started this flow.
#[utoipa::path(
    get,
    path = "/auth/oauth/authorize",
    responses(
        (status = 302, description = "Redirect to the identity provider's authorization endpoint, with a signed `state` and PKCE challenge attached. Also sets the `ysr_oauth_csrf` cookie the callback checks `state` against"),
        (status = 404, description = "OAuth is not configured (`YORISHIRO_OAUTH_ISSUER_URL` unset)", body = crate::error::HostedApiErrorBody),
        (status = 429, description = "Per-IP rate limit exhausted: shares one quota with the community server's own `/auth/login`/`/auth/signup`/`/setup*`. Returned with no JSON body"),
        (status = 500, description = "Discovery document or JWKS fetch failed", body = crate::error::HostedApiErrorBody),
    ),
    tag = "hosted-oauth",
)]
pub async fn authorize(
    State(state): State<HostedState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, HostedApiError> {
    let config = state.oauth_config.as_ref().ok_or_else(not_found)?;

    let redirect = oauth::build_authorize_redirect(config)
        .await
        .inspect_err(|err| tracing::error!(error = %err, "failed to build OAuth authorize URL"))?;

    let csrf_cookie = Cookie::build((CSRF_COOKIE_NAME, redirect.csrf_cookie_value))
        .http_only(true)
        .secure(config.cookies_require_secure())
        .same_site(SameSite::Lax)
        .path("/auth/oauth/callback")
        .max_age(Duration::seconds(oauth::STATE_TTL_SECS))
        .build();

    Ok((jar.add(csrf_cookie), Redirect::to(&redirect.url)))
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// `GET /auth/oauth/callback`: the identity provider's redirect target.
/// Exchanges the authorization code for tokens, verifies the ID token, resolves (or auto-provisions) the Yorishiro user/tenant/workspace it corresponds to, issues an API key exactly the way `POST /auth/login` does, and hands it to the Web UI via a URL fragment (`#api_key=...`), a fragment rather than a query parameter so the key never appears in server access logs or gets sent back to any server in a `Referer` header.
#[utoipa::path(
    get,
    path = "/auth/oauth/callback",
    params(
        ("code" = Option<String>, Query, description = "Authorization code from the identity provider"),
        ("state" = Option<String>, Query, description = "The signed state issued by `/auth/oauth/authorize`"),
        ("error" = Option<String>, Query, description = "Set when the provider itself rejected the login"),
        ("error_description" = Option<String>, Query, description = "Human-readable detail accompanying `error`"),
    ),
    responses(
        (status = 302, description = "On success, redirect to `/#api_key=<key>`. Two failure modes also redirect here instead of returning JSON, because the caller is a browser mid-redirect: the provider returning `error=...`, and a callback missing `code`/`state` (both go to `/#/login?error=oauth_failed`)"),
        (status = 401, description = "State signature, freshness, CSRF cookie, token exchange or ID-token verification failed", body = crate::error::HostedApiErrorBody),
        (status = 403, description = "A returning identity whose tenant membership no longer checks out", body = crate::error::HostedApiErrorBody),
        (status = 404, description = "OAuth is not configured (`YORISHIRO_OAUTH_ISSUER_URL` unset)", body = crate::error::HostedApiErrorBody),
        (status = 409, description = "The ID token's email is already registered under a different provider/subject", body = crate::error::HostedApiErrorBody),
        (status = 422, description = "The provider omitted the `email` claim, so no account can be auto-provisioned", body = crate::error::HostedApiErrorBody),
        (status = 429, description = "Per-IP rate limit exhausted, checked before any state/CSRF validation. Returned with no JSON body"),
        (status = 500, description = "Identity-provider communication failed, or the per-identity provisioning lock timed out", body = crate::error::HostedApiErrorBody),
    ),
    tag = "hosted-oauth",
)]
pub async fn callback(
    State(state): State<HostedState>,
    jar: CookieJar,
    Query(params): Query<CallbackParams>,
) -> Result<Response, HostedApiError> {
    let config = state.oauth_config.as_ref().ok_or_else(not_found)?;

    // Read before removing below: `CookieJar::remove` marks the cookie as removed in the same jar it was read from, so `get` would see nothing afterward.
    let csrf_cookie_value = jar.get(CSRF_COOKIE_NAME).map(|c| c.value().to_string());

    // Cleared on every path that completes this flow attempt (both explicit failure redirects below, and success at the end) so the cookie is never reusable across two attempts by the same browser.
    // The `?`-propagated error paths below (provider/DB failures) don't go through this `jar`, so on those the cookie is simply left to expire on its own at `oauth::STATE_TTL_SECS`: a harmless, short-lived leftover, not a reusable credential (the `state` it was bound to is single-use and already consumed or invalid by then).
    let jar = jar.remove(Cookie::from(CSRF_COOKIE_NAME));

    if let Some(error) = params.error {
        tracing::warn!(
            error,
            description = params.error_description.as_deref().unwrap_or_default(),
            "identity provider returned an error on the OAuth callback"
        );
        return Ok((jar, login_failure_redirect()).into_response());
    }

    let (Some(code), Some(request_state)) = (params.code, params.state) else {
        return Ok((jar, login_failure_redirect()).into_response());
    };

    let identity =
        oauth::handle_callback(config, &code, &request_state, csrf_cookie_value.as_deref()).await?;

    let provisioned = oauth::find_or_create(
        &state.identity_pool,
        "oidc",
        &identity.subject_id,
        identity.email.as_deref(),
        identity.display_name.as_deref(),
    )
    .await?;

    let mut conn = state.identity_pool.acquire().await.internal()?;
    let created = auth::create_api_key(
        &mut conn,
        provisioned.workspace_id,
        provisioned.role.max_scope(),
        Some(provisioned.user_id),
    )
    .await?;

    Ok((jar, login_success_redirect(&created.plaintext)).into_response())
}

fn login_success_redirect(api_key: &str) -> Response {
    // `api_key` is a value this process just generated (`ysr_<hex>_<hex>`, see `services::auth::create_api_key`), not provider- or user-supplied input, so no additional encoding is needed for it to appear safely in a URL fragment.
    // The fragment (rather than a query parameter) is what keeps the key out of server access logs and any `Referer` header a subsequent same-page navigation might send.
    // See `app.js`'s `router()`, which looks for `#api_key=...` specifically to detect this redirect.
    let location = format!("/#api_key={api_key}");
    (StatusCode::FOUND, [(header::LOCATION, location)], ()).into_response()
}

fn login_failure_redirect() -> Response {
    let location = "/#/login?error=oauth_failed".to_string();
    (StatusCode::FOUND, [(header::LOCATION, location)], ()).into_response()
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/oauth.rs"]
mod tests;
