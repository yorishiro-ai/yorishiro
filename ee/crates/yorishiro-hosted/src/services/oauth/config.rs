//! Configuration for OAuth2/OIDC login, read fresh from the environment on every request (see `controllers::oauth`'s module doc comment for why).
//! A bad config reports `YorishiroError::Internal` rather than panicking, since a panic here would crash the task handling whichever request triggered it.
//!
//! `Ok(None)` (via [`OAuthConfig::from_env`]) means OAuth is disabled.
//! Every `/auth/oauth/authorize` and `/auth/oauth/callback` request then answers `404`, and the community server's own `/auth/login` (email/password) is unaffected.

use yorishiro_core::YorishiroError;

use crate::services::non_empty_env;

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// The identity provider's issuer URL, e.g. `https://accounts.google.com`.
    /// OIDC discovery is fetched from this at request time, not cached at startup.
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// Where the provider redirects back to after the user authenticates.
    /// Defaults to a `localhost`-rewritten `YORISHIRO_BIND`, see [`default_redirect_uri`]: a bind address is usually `0.0.0.0:...`, not a host a browser can reach.
    pub redirect_uri: String,
    /// HMAC key used to sign the `state` parameter that round-trips through the provider.
    /// Derived from `client_secret` so no separate secret needs provisioning.
    pub state_signing_key: Vec<u8>,
}

impl OAuthConfig {
    /// Reads the four `YORISHIRO_OAUTH_*` variables.
    /// Returns `Ok(None)` when `YORISHIRO_OAUTH_ISSUER_URL` is unset or empty: OAuth login is opt-in, and every other variable is meaningless without an issuer to talk to.
    /// `YORISHIRO_OAUTH_CLIENT_ID`/`YORISHIRO_OAUTH_CLIENT_SECRET` are required once the issuer is set; a deployment that sets the issuer but leaves one of these unset or empty gets `Err` naming which, rather than silently leaving OAuth half-configured or being treated the same as simply unconfigured.
    pub fn from_env() -> Result<Option<Self>, YorishiroError> {
        let Some(issuer_url) = non_empty_env("YORISHIRO_OAUTH_ISSUER_URL") else {
            return Ok(None);
        };

        let client_id = require_non_empty_env("YORISHIRO_OAUTH_CLIENT_ID")?;
        let client_secret = require_non_empty_env("YORISHIRO_OAUTH_CLIENT_SECRET")?;

        let redirect_uri =
            non_empty_env("YORISHIRO_OAUTH_REDIRECT_URI").unwrap_or_else(default_redirect_uri);

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
    pub fn cookies_require_secure(&self) -> bool {
        self.redirect_uri.starts_with("https://")
    }
}

/// Reads `key` via [`non_empty_env`] or errors naming exactly which variable is missing and why it matters.
fn require_non_empty_env(key: &str) -> Result<String, YorishiroError> {
    require_non_empty(key, non_empty_env(key).as_deref())
}

/// The pure fold `require_non_empty_env` wraps, split out so tests can exercise every case
/// (unset, set-but-empty, set) without mutating the process environment.
pub fn require_non_empty(key: &str, raw: Option<&str>) -> Result<String, YorishiroError> {
    match raw.filter(|s| !s.is_empty()) {
        Some(value) => Ok(value.to_string()),
        None => Err(YorishiroError::Internal(anyhow::anyhow!(
            "{key} must be set to a non-empty value when YORISHIRO_OAUTH_ISSUER_URL is"
        ))),
    }
}

/// `http://{host}/auth/oauth/callback`, where `host` rewrites an all-interfaces `YORISHIRO_BIND` to `localhost` since a browser cannot dial `0.0.0.0` directly.
/// This default only covers local testing; a real deployment sets `YORISHIRO_OAUTH_REDIRECT_URI` explicitly.
fn default_redirect_uri() -> String {
    let bind = std::env::var("YORISHIRO_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_non_empty_accepts_a_present_value() {
        assert_eq!(require_non_empty("KEY", Some("value")).unwrap(), "value");
    }

    #[test]
    fn require_non_empty_rejects_an_unset_value() {
        let err = require_non_empty("KEY", None).unwrap_err();
        assert!(
            err.to_string()
                .contains("KEY must be set to a non-empty value")
        );
    }

    #[test]
    fn require_non_empty_rejects_a_set_but_empty_value() {
        let err = require_non_empty("KEY", Some("")).unwrap_err();
        assert!(
            err.to_string()
                .contains("KEY must be set to a non-empty value")
        );
    }

    #[test]
    fn rewrite_unspecified_host_rewrites_all_interfaces_addresses() {
        assert_eq!(rewrite_unspecified_host("0.0.0.0:8080"), "localhost:8080");
        assert_eq!(rewrite_unspecified_host("[::]:8080"), "localhost:8080");
    }

    #[test]
    fn rewrite_unspecified_host_leaves_a_real_address_alone() {
        // Must not be corrupted by a substring replace: this merely contains "0.0.0.0".
        assert_eq!(rewrite_unspecified_host("10.0.0.0:8081"), "10.0.0.0:8081");
        assert_eq!(rewrite_unspecified_host("127.0.0.1:8080"), "127.0.0.1:8080");
    }

    #[test]
    fn rewrite_unspecified_host_leaves_a_non_socket_addr_alone() {
        assert_eq!(
            rewrite_unspecified_host("example.com:8080"),
            "example.com:8080"
        );
    }
}
