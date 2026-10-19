//! Configuration for OAuth2/OIDC login from resolved Loco initializer settings.
//! A bad config reports `YorishiroError::Internal` rather than panicking, since a panic here would crash the task handling whichever request triggered it.
//!
//! `Ok(None)` (via [`OAuthConfig::from_config`]) means OAuth is disabled.
//! Every `/auth/oauth/authorize` and `/auth/oauth/callback` request then answers `404`, and the community server's own `/auth/login` (email/password) is unaffected.

use loco_rs::config::{Config, Server};
use serde::Deserialize;

use crate::YorishiroError;

#[derive(Clone)]
pub struct OAuthConfig {
    /// The identity provider's issuer URL, e.g. `https://accounts.google.com`.
    /// OIDC discovery is fetched from this at request time, not cached at startup.
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// Where the provider redirects back to after the user authenticates.
    /// Defaults to the configured server binding and port, rewriting an all-interfaces binding to `localhost` for local testing.
    pub redirect_uri: String,
    /// HMAC key used to sign the `state` parameter that round-trips through the provider.
    /// Derived from `client_secret` so no separate secret needs provisioning.
    pub state_signing_key: Vec<u8>,
}

impl OAuthConfig {
    /// Reads `initializers.oauth` and the resolved listener for a local callback fallback.
    /// Returns `Ok(None)` when `YORISHIRO_OAUTH_ISSUER_URL` is unset or empty: OAuth login is opt-in, and every other variable is meaningless without an issuer to talk to.
    /// `YORISHIRO_OAUTH_CLIENT_ID`/`YORISHIRO_OAUTH_CLIENT_SECRET` are required once the issuer is set; a deployment that sets the issuer but leaves one of these unset or empty gets `Err` naming which, rather than silently leaving OAuth half-configured or being treated the same as simply unconfigured.
    pub(crate) fn from_config(config: &Config) -> Result<Option<Self>, YorishiroError> {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Settings {
            issuer_url: Option<String>,
            client_id: Option<String>,
            client_secret: Option<String>,
            redirect_uri: Option<String>,
            bind: Option<String>,
        }
        let settings: Settings = config
            .initializers
            .as_ref()
            .and_then(|values| values.get("oauth"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "invalid initializers.oauth configuration"
                ))
            })?
            .unwrap_or_default();
        let Some(issuer_url) = settings.issuer_url.filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let client_id =
            require_non_empty("YORISHIRO_OAUTH_CLIENT_ID", settings.client_id.as_deref())?;
        let client_secret = require_non_empty(
            "YORISHIRO_OAUTH_CLIENT_SECRET",
            settings.client_secret.as_deref(),
        )?;
        let redirect_uri = settings
            .redirect_uri
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                settings
                    .bind
                    .filter(|value| !value.is_empty())
                    .map(|bind| {
                        format!(
                            "http://{}/auth/oauth/callback",
                            rewrite_unspecified_host(&bind)
                        )
                    })
                    .unwrap_or_else(|| default_redirect_uri(&config.server))
            });

        let state_signing_key = client_secret.as_bytes().to_vec();

        Ok(Some(Self {
            issuer_url: issuer_url.trim_end_matches('/').to_string(),
            client_id,
            client_secret,
            redirect_uri,
            state_signing_key,
        }))
    }

    /// Whether the CSRF cookie `authorize` sets should carry the `Secure` attribute.
    /// Tied to `redirect_uri`'s scheme rather than a separate variable.
    pub(crate) fn cookies_require_secure(&self) -> bool {
        self.redirect_uri.starts_with("https://")
    }
}

/// Validates a required OAuth credential without exposing its value.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub fn require_non_empty(key: &str, raw: Option<&str>) -> Result<String, YorishiroError> {
    match raw.filter(|s| !s.is_empty()) {
        Some(value) => Ok(value.to_string()),
        None => Err(YorishiroError::Internal(anyhow::anyhow!(
            "{key} must be set to a non-empty value when YORISHIRO_OAUTH_ISSUER_URL is"
        ))),
    }
}

/// `http://{host}/auth/oauth/callback`, using the server's actual binding and port using the resolved listener.
/// This default only covers local testing; a real deployment sets `YORISHIRO_OAUTH_REDIRECT_URI` explicitly.
pub fn default_redirect_uri(server: &Server) -> String {
    let binding = &server.binding;
    let binding = if binding.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{binding}]")
    } else {
        binding.clone()
    };
    let bind = format!("{binding}:{}", server.port);
    format!(
        "http://{}/auth/oauth/callback",
        rewrite_unspecified_host(&bind)
    )
}

/// Rewrites `host:port` to `localhost:port` when the host is an all-interfaces bind address (`0.0.0.0` or `::`), leaving everything else as given.
/// Parses the whole string as a [`std::net::SocketAddr`] rather than doing a substring replace, which would corrupt an address like `10.0.0.0:8081` into `1localhost:8081`.
/// A `bind` that is not a valid `SocketAddr` at all passes through unchanged.
pub fn rewrite_unspecified_host(bind: &str) -> String {
    match bind.parse::<std::net::SocketAddr>() {
        Ok(addr) if addr.ip().is_unspecified() => format!("localhost:{}", addr.port()),
        _ => bind.to_string(),
    }
}
