//! `GET /auth/oauth/status|authorize|callback`: OAuth2/OIDC login, an additional way to obtain a Yorishiro API key alongside the community server's own `POST /auth/login`.
//!
//! All three routes carry the licence gate, so an unlicensed deployment answers `404` to every one of them before any of the below runs.
//! SSO login is a paid-edition feature, which is a decision about what the feature is; needing configuration on top of that does not make it a community route.
//!
//! They are also opt-in within a licensed deployment: `OAuthConfig::from_env()` resolves to `Ok(None)` unless `YORISHIRO_OAUTH_ISSUER_URL` is set, in which case `authorize`/`callback` return `404 Not Found` before doing anything else, indistinguishable from the route simply not existing.
//! A set issuer with a missing `client_id`/`client_secret` is a different case, `Err`, and answers `500` naming the misconfiguration rather than either `404`.
//! `status` is the exception and always answers `200` once past the gate, which is what makes it the route `licence_gate.rs` tests the gate with: its `404` can only come from the gate.

use crate::YorishiroError;
use crate::controllers::ApiError;
use crate::error::ResultExt;
use crate::models::identity_api_keys::IdentityApiKeys;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;
use serde::{Deserialize, Serialize};

use crate::ee::services::oauth;
use crate::ee::services::oauth::OAuthConfig;

/// Name of the CSRF cookie `authorize` sets and `callback` reads back.
const CSRF_COOKIE_NAME: &str = "ysr_oauth_csrf";

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
    let max_age = oauth::STATE_TTL_SECS;
    format!(
        "{CSRF_COOKIE_NAME}={value}; HttpOnly; SameSite=Lax; Path=/auth/oauth/callback; \
         Max-Age={max_age}{secure_attr}",
    )
}

/// Clears the CSRF cookie: `Max-Age=0` on the same path it was set with, so the browser drops it immediately rather than leaving it to expire naturally.
fn csrf_clear_cookie() -> String {
    format!("{CSRF_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/auth/oauth/callback; Max-Age=0")
}

#[derive(Debug, Serialize)]
pub struct OAuthStatus {
    /// `false` when `YORISHIRO_OAUTH_ISSUER_URL` is unset, in which case `authorize`/`callback` return `404`.
    pub enabled: bool,
}

/// `GET /auth/oauth/status`: lets a client decide whether to show the "Sign in with SSO" button, without hardcoding a build-time assumption about whether OAuth is configured.
/// Unlike the other two routes, this one always returns `200` (`enabled: false` when unconfigured) rather than `404`, since a client that could not tell "not configured" apart from "not present" would have no way to decide whether to show the button at all.
async fn status() -> Result<Json<OAuthStatus>, ApiError> {
    Ok(Json(OAuthStatus {
        enabled: OAuthConfig::from_env()?.is_some(),
    }))
}

/// `GET /auth/oauth/authorize`: starts the login flow by redirecting the browser to the identity provider's own authorization endpoint.
/// Also sets the CSRF cookie that `callback` checks the returning `state` against: without it, `state`'s HMAC signature alone only proves this server issued *some* state, not that the browser presenting it at callback time is the one that started this flow.
async fn authorize() -> Result<Response, ApiError> {
    let config = OAuthConfig::from_env()?.ok_or_else(not_found)?;

    let redirect = oauth::build_authorize_redirect(&config)
        .await
        .inspect_err(|err| tracing::error!(error = %err, "failed to build OAuth authorize URL"))?;

    let set_cookie = csrf_set_cookie(&redirect.csrf_cookie_value, config.cookies_require_secure());

    Ok((
        [(header::SET_COOKIE, set_cookie)],
        Redirect::to(&redirect.url),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// `GET /auth/oauth/callback`: the identity provider's redirect target.
/// Exchanges the authorization code for tokens, verifies the ID token, resolves (or auto-provisions) the Yorishiro user/tenant/workspace it corresponds to, issues an API key exactly the way `POST /auth/login` does, and hands it to the client via a URL fragment (`#api_key=...`), a fragment rather than a query parameter so the key never appears in server access logs or gets sent back to any server in a `Referer` header.
async fn callback(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Result<Response, ApiError> {
    let config = OAuthConfig::from_env()?.ok_or_else(not_found)?;

    let csrf_cookie_value = read_cookie(&headers, CSRF_COOKIE_NAME);
    let clear_cookie = csrf_clear_cookie();

    if let Some(error) = params.error {
        tracing::warn!(
            error,
            description = params.error_description.as_deref().unwrap_or_default(),
            "identity provider returned an error on the OAuth callback"
        );
        return Ok(login_failure_redirect(clear_cookie));
    }

    let (Some(code), Some(request_state)) = (params.code, params.state) else {
        return Ok(login_failure_redirect(clear_cookie));
    };

    let identity =
        oauth::handle_callback(&config, &code, &request_state, csrf_cookie_value.as_deref())
            .await?;

    // The deployment's actual embedding model and width, the same source `setup.rs` stamps a freshly bootstrapped workspace with, not a guessed default: `content_entities.embedding`'s index is a fixed width, and a workspace stamped with the wrong one fails every entity write's dimension check.
    let embedding_provider = ctx
        .shared_store
        .get::<std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>>()
        .ok_or_else(|| {
            ApiError(YorishiroError::Internal(anyhow::anyhow!(
                "EmbeddingProvider missing"
            )))
        })?;
    let embedding_model = crate::services::embedding::model_name_from_env();
    let embedding_dimensions = embedding_provider.dimensions() as i32;

    let txn = ctx.db.begin().await.internal()?;
    let provisioned = oauth::find_or_create(
        &txn,
        "oidc",
        &identity.subject_id,
        identity.email.as_deref(),
        identity.display_name.as_deref(),
        (&embedding_model, embedding_dimensions),
    )
    .await?;
    txn.commit().await.internal()?;

    let created = IdentityApiKeys::create_api_key(
        &ctx.db,
        provisioned.workspace_id,
        provisioned.role.max_scope(),
        Some(provisioned.user_id),
        false,
    )
    .await?;

    Ok(login_success_redirect(&created.plaintext, clear_cookie))
}

fn login_success_redirect(api_key: &str, clear_cookie: String) -> Response {
    // `api_key` is a value this process just generated (`ysr_<hex>_<hex>`, see `crate::services::auth::create_api_key`), not provider- or user-supplied input, so no additional encoding is needed for it to appear safely in a URL fragment.
    let location = format!("/#api_key={api_key}");
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, location),
            (header::SET_COOKIE, clear_cookie),
        ],
    )
        .into_response()
}

fn login_failure_redirect(clear_cookie: String) -> Response {
    let location = "/#/login?error=oauth_failed".to_string();
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, location),
            (header::SET_COOKIE, clear_cookie),
        ],
    )
        .into_response()
}

pub fn routes() -> Routes {
    use axum::routing::get;

    Routes::new()
        .prefix("auth/oauth")
        .add("/status", get(status))
        .add("/authorize", get(authorize))
        .add("/callback", get(callback))
}
