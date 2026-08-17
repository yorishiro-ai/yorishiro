//! OAuth2/OIDC login: an additional, optional way to obtain a Yorishiro API key alongside the
//! embedded community server's own `POST /auth/login` (email/password). See `config::OAuthConfig`
//! for how this is enabled/disabled, and `http::controllers::oauth` for the two routes that use
//! this module.

pub mod config;
mod discovery;
mod id_token;
mod state_token;
mod users;

pub use config::{OAuthConfig, require_non_empty, rewrite_unspecified_host};
pub use state_token::STATE_TTL_SECS;
pub use users::{
    CreateOauthUserError, OAuthUser, ProvisionedLogin, create_oauth_user, find_or_create,
};

use yorishiro_core::YorishiroError;

/// Everything `GET /auth/oauth/authorize` needs to build its redirect and set the CSRF cookie
/// that binds the flow to this browser (see `state_token` module docs).
pub struct AuthorizeRedirect {
    pub url: String,
    pub csrf_cookie_value: String,
}

/// Builds the provider's authorize URL (RFC 6749 §4.1.1) with a freshly issued, signed `state`
/// (see `state_token`) and PKCE challenge (RFC 7636) attached. `openid email profile` is a fixed
/// scope request rather than configurable, since `email` is the one claim this integration
/// actually requires (see `users::find_or_create`) and every OIDC provider recognizes all three.
pub async fn build_authorize_redirect(
    config: &OAuthConfig,
) -> Result<AuthorizeRedirect, YorishiroError> {
    let discovery = discovery::fetch_discovery_document(&config.issuer_url).await?;
    let issued = state_token::issue(&config.state_signing_key);

    let url = url_with_query(
        &discovery.authorization_endpoint,
        &[
            ("response_type", "code"),
            ("client_id", &config.client_id),
            ("redirect_uri", &config.redirect_uri),
            ("scope", "openid email profile"),
            ("state", &issued.state),
            ("code_challenge", &issued.pkce_challenge),
            ("code_challenge_method", "S256"),
        ],
    )?;

    Ok(AuthorizeRedirect {
        url,
        csrf_cookie_value: issued.csrf_cookie_value,
    })
}

/// The verified result of a callback: the identity provider's subject id/email/display name,
/// ready to be handed to `users::find_or_create`.
pub struct CallbackIdentity {
    pub subject_id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

/// Handles `GET /auth/oauth/callback`'s core logic: verifies `state` and the CSRF cookie it's
/// bound to (see `state_token` module docs), exchanges `code` for tokens, and verifies the
/// returned ID token. Split out from the controller so it has no axum dependency and can be
/// unit/integration tested without spinning up a router.
///
/// `csrf_cookie_value` is `None` when the browser presented no CSRF cookie at all (e.g. it was
/// stripped, or the callback is being hit without ever having visited `/auth/oauth/authorize`
/// first): treated the same as a mismatched cookie, both rejected before `state` is trusted.
pub async fn handle_callback(
    config: &OAuthConfig,
    code: &str,
    state: &str,
    csrf_cookie_value: Option<&str>,
) -> Result<CallbackIdentity, YorishiroError> {
    let verified = state_token::verify(&config.state_signing_key, state).ok_or_else(|| {
        tracing::warn!("OAuth callback rejected: invalid or expired state parameter");
        YorishiroError::Unauthenticated
    })?;

    let cookie_hash_matches = csrf_cookie_value
        .map(state_token::hash_csrf_cookie)
        .is_some_and(|hash| hash == verified.csrf_hash);
    if !cookie_hash_matches {
        tracing::warn!(
            "OAuth callback rejected: CSRF cookie missing or did not match the state parameter"
        );
        return Err(YorishiroError::Unauthenticated);
    }
    let pkce_verifier = verified.pkce_verifier;

    let discovery = discovery::fetch_discovery_document(&config.issuer_url).await?;

    let tokens = discovery::exchange_code_for_tokens(
        &discovery.token_endpoint,
        &config.client_id,
        &config.client_secret,
        code,
        &config.redirect_uri,
        &pkce_verifier,
    )
    .await?;

    let jwks = discovery::fetch_jwks(&discovery.jwks_uri).await?;
    let claims = id_token::verify(
        &tokens.id_token,
        &jwks,
        &config.issuer_url,
        &config.client_id,
    )?;

    if claims.email.is_some() && !claims.email_verified {
        tracing::warn!(
            subject = %claims.sub,
            "OAuth ID token has an email claim that is not marked verified; proceeding, since \
             some providers omit email_verified for accounts that are inherently verified \
             (e.g. enterprise SSO)"
        );
    }

    Ok(CallbackIdentity {
        subject_id: claims.sub,
        email: claims.email,
        display_name: claims.name,
    })
}

/// Builds `base` (the provider's discovery document's `authorization_endpoint`, fetched live on
/// every `/auth/oauth/authorize` request: see `discovery::fetch_discovery_document`) with
/// `params` appended as a query string. Returns an error rather than panicking on a malformed
/// `base`, since it comes from an external, unvalidated network response: a misconfigured or
/// misbehaving provider must not be able to crash the request-handling task.
fn url_with_query(base: &str, params: &[(&str, &str)]) -> Result<String, YorishiroError> {
    let mut url = url::Url::parse(base).map_err(|err| {
        YorishiroError::Internal(anyhow::anyhow!(
            "provider's authorization_endpoint '{base}' is not a valid URL: {err}"
        ))
    })?;
    for (key, value) in params {
        url.query_pairs_mut().append_pair(key, value);
    }
    Ok(url.to_string())
}

#[cfg(test)]
#[path = "../../../tests/services/oauth/mod.rs"]
mod tests;
