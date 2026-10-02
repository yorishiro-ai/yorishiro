//! OAuth2/OIDC login: an additional, optional way to obtain a Yorishiro API key alongside the community server's own `POST /auth/login` (email/password).
//! See `config::OAuthConfig` for how this is enabled/disabled, and `controllers::oauth` for the three routes that use this module.

pub mod config;
mod discovery;
mod id_token;
mod state_token;
mod users;

pub use config::OAuthConfig;
pub use state_token::STATE_TTL_SECS;
pub use users::{ProvisionedLogin, find_or_create};

use std::sync::Arc;

use crate::YorishiroError;

/// Everything `GET /auth/oauth/authorize` needs to build its redirect and set the CSRF cookie that binds the flow to this browser (see `state_token` module docs).
pub struct AuthorizeRedirect {
    pub url: String,
    pub csrf_cookie_value: String,
}

/// Builds the provider's authorize URL (the OAuth 2.0 authorize endpoint) with a freshly issued, signed `state` (see `state_token`) and PKCE challenge (RFC 7636) attached.
/// `openid email profile` is a fixed scope request, not configurable: `email` is the one claim `users::find_or_create` requires.
pub async fn build_authorize_redirect(
    config: &OAuthConfig,
) -> Result<AuthorizeRedirect, YorishiroError> {
    build_authorize_redirect_with_http(config, discovery::production()).await
}

async fn build_authorize_redirect_with_http(
    config: &OAuthConfig,
    http: Arc<dyn discovery::OAuthHttp>,
) -> Result<AuthorizeRedirect, YorishiroError> {
    let discovery = http.fetch_discovery_document(&config.issuer_url).await?;
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

/// The verified result of a callback: the identity provider's subject id/email/display name, ready to be handed to `users::find_or_create`.
pub struct CallbackIdentity {
    pub subject_id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

/// Handles `GET /auth/oauth/callback`'s core logic: verifies `state` and the CSRF cookie it is bound to (see `state_token` module docs), exchanges `code` for tokens, and verifies the returned ID token.
///
/// `csrf_cookie_value` is `None` when the browser presented no CSRF cookie at all: treated the same as a mismatched cookie, both rejected before `state` is trusted.
pub async fn handle_callback(
    config: &OAuthConfig,
    code: &str,
    state: &str,
    csrf_cookie_value: Option<&str>,
) -> Result<CallbackIdentity, YorishiroError> {
    handle_callback_with_http(
        config,
        code,
        state,
        csrf_cookie_value,
        discovery::production(),
    )
    .await
}

async fn handle_callback_with_http(
    config: &OAuthConfig,
    code: &str,
    state: &str,
    csrf_cookie_value: Option<&str>,
    http: Arc<dyn discovery::OAuthHttp>,
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

    let discovery = http.fetch_discovery_document(&config.issuer_url).await?;

    let tokens = http
        .exchange_code_for_tokens(
            &discovery.token_endpoint,
            &config.client_id,
            &config.client_secret,
            code,
            &config.redirect_uri,
            &pkce_verifier,
        )
        .await?;

    let jwks = http.fetch_jwks(&discovery.jwks_uri).await?;
    let claims = id_token::verify(
        &tokens.id_token,
        &jwks,
        &config.issuer_url,
        &config.client_id,
    )?;

    if claims.email.is_some() && !claims.email_verified {
        tracing::warn!(
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

/// Builds `base` (the provider's `authorization_endpoint`) with `params` appended as a query string.
/// Returns an error rather than panicking on a malformed `base`, since it comes from an external, unvalidated network response.
fn url_with_query(base: &str, params: &[(&str, &str)]) -> Result<String, YorishiroError> {
    let mut url = reqwest::Url::parse(base).map_err(|err| {
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
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use jsonwebtoken::jwk::JwkSet;

    use super::*;

    struct FakeOAuthHttp {
        calls: Mutex<Vec<&'static str>>,
    }

    #[async_trait]
    impl discovery::OAuthHttp for FakeOAuthHttp {
        async fn fetch_discovery_document(
            &self,
            _issuer_url: &str,
        ) -> Result<discovery::DiscoveryDocument, YorishiroError> {
            self.calls.lock().unwrap().push("discovery");
            Ok(discovery::DiscoveryDocument {
                authorization_endpoint: "https://idp.example/authorize".into(),
                token_endpoint: "https://idp.example/token".into(),
                jwks_uri: "https://idp.example/jwks".into(),
            })
        }

        async fn exchange_code_for_tokens(
            &self,
            _token_endpoint: &str,
            _client_id: &str,
            _client_secret: &str,
            _code: &str,
            _redirect_uri: &str,
            _pkce_verifier: &str,
        ) -> Result<discovery::TokenResponse, YorishiroError> {
            self.calls.lock().unwrap().push("token");
            Ok(discovery::TokenResponse {
                id_token: "not-a-jwt".into(),
            })
        }

        async fn fetch_jwks(&self, _jwks_uri: &str) -> Result<JwkSet, YorishiroError> {
            self.calls.lock().unwrap().push("jwks");
            Ok(JwkSet { keys: Vec::new() })
        }
    }

    fn config() -> OAuthConfig {
        OAuthConfig {
            issuer_url: "https://idp.example".into(),
            client_id: "client-id".into(),
            client_secret: "client-secret".into(),
            redirect_uri: "http://localhost:8080/auth/oauth/callback".into(),
            state_signing_key: b"client-secret".to_vec(),
        }
    }

    #[tokio::test]
    async fn authorize_uses_the_injected_discovery_operation() {
        let http = Arc::new(FakeOAuthHttp {
            calls: Mutex::new(Vec::new()),
        });
        let redirect = build_authorize_redirect_with_http(&config(), http.clone())
            .await
            .expect("fake discovery should build the authorization redirect");

        assert!(redirect.url.starts_with("https://idp.example/authorize?"));
        assert!(redirect.url.contains("code_challenge_method=S256"));
        assert!(!redirect.csrf_cookie_value.is_empty());
        assert_eq!(*http.calls.lock().unwrap(), vec!["discovery"]);
    }

    #[tokio::test]
    async fn callback_uses_all_three_injected_operations_before_token_verification() {
        let http = Arc::new(FakeOAuthHttp {
            calls: Mutex::new(Vec::new()),
        });
        let issued = state_token::issue(b"client-secret");
        let result = handle_callback_with_http(
            &config(),
            "authorization-code",
            &issued.state,
            Some(&issued.csrf_cookie_value),
            http.clone(),
        )
        .await;
        let error = match result {
            Ok(_) => panic!("the deliberately invalid ID token must be rejected"),
            Err(error) => error,
        };

        assert!(matches!(error, YorishiroError::Unauthenticated));
        assert_eq!(
            *http.calls.lock().unwrap(),
            vec!["discovery", "token", "jwks"]
        );
    }
}
