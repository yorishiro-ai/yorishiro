/// Configuration for OAuth2/OIDC login, read once at startup from the environment. `None` (via
/// [`OAuthConfig::from_env`] returning `None`) means OAuth is disabled: every `/auth/oauth/*`
/// route then returns `404 Not Found`, and the existing `/auth/login` (email/password, from the
/// embedded community server) is completely unaffected. This mirrors how `StripeConfig` gates
/// the Stripe webhook: an optional add-on the enterprise binary carries even when unconfigured,
/// rather than a compile-time feature flag.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// The identity provider's issuer URL, e.g. `https://accounts.google.com` or
    /// `https://login.microsoftonline.com/{tenant}/v2.0`. OIDC discovery
    /// (`{issuer_url}/.well-known/openid-configuration`) is fetched from this at request time
    /// (not cached at startup) so a provider that rotates its signing keys or endpoints doesn't
    /// require a restart.
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// Where the provider redirects back to after the user authenticates. Defaults to
    /// `{YORISHIRO_BIND}/auth/oauth/callback` reinterpreted as a public-facing URL isn't
    /// possible from the bind address alone (it's usually `0.0.0.0:...`, not a reachable host),
    /// so this has its own explicit default derived from a public base URL instead, see
    /// [`OAuthConfig::from_env`].
    pub redirect_uri: String,
    /// HMAC key used to sign the `state` parameter that round-trips through the provider
    /// (see `services::oauth::state_token`). Derived from `client_secret` so no separate secret
    /// needs to be provisioned: the state token's only job is proving this process issued it
    /// (CSRF protection for the callback), not protecting data confidentiality.
    pub state_signing_key: Vec<u8>,
}

impl OAuthConfig {
    /// Reads the four `YORISHIRO_OAUTH_*` variables. Returns `None` when
    /// `YORISHIRO_OAUTH_ISSUER_URL` is unset or empty: OAuth login is opt-in, and every other
    /// variable is meaningless without an issuer to talk to. `YORISHIRO_OAUTH_CLIENT_ID`/
    /// `YORISHIRO_OAUTH_CLIENT_SECRET` are required once the issuer is set (a provider can't be
    /// used anonymously); a deployment that sets the issuer but leaves one of these unset *or
    /// empty* fails fast at startup, the same way `DATABASE_URL` does in `main`, rather than
    /// silently leaving OAuth half-configured: an empty `client_secret` in particular would
    /// otherwise become an empty [`OAuthConfig::state_signing_key`], the HMAC key that signs the
    /// CSRF `state` token.
    pub fn from_env() -> Option<Self> {
        let issuer_url = crate::services::non_empty_env("YORISHIRO_OAUTH_ISSUER_URL")?;

        let client_id = require_non_empty_env("YORISHIRO_OAUTH_CLIENT_ID");
        let client_secret = require_non_empty_env("YORISHIRO_OAUTH_CLIENT_SECRET");

        let redirect_uri = crate::services::non_empty_env("YORISHIRO_OAUTH_REDIRECT_URI")
            .unwrap_or_else(default_redirect_uri);

        let state_signing_key = client_secret.as_bytes().to_vec();

        Some(Self {
            issuer_url: issuer_url.trim_end_matches('/').to_string(),
            client_id,
            client_secret,
            redirect_uri,
            state_signing_key,
        })
    }

    /// Whether the CSRF cookie `http::controllers::oauth::authorize` sets should carry the
    /// `Secure` attribute. Tied to `redirect_uri`'s scheme rather than a separate env var: a
    /// `Secure` cookie is never sent back to a plain-`http://` callback, so setting it
    /// unconditionally would silently break the flow for the `http://localhost:...` default
    /// (`default_redirect_uri`) that same-host/local testing relies on, while a real deployment
    /// (which sets `YORISHIRO_OAUTH_REDIRECT_URI` to a public `https://` URL) gets the
    /// stricter attribute automatically.
    pub fn cookies_require_secure(&self) -> bool {
        self.redirect_uri.starts_with("https://")
    }
}

/// Reads `key` via [`crate::services::non_empty_env`] or panics naming exactly which variable
/// is missing/empty and why it matters: used for `YORISHIRO_OAUTH_CLIENT_ID`/
/// `YORISHIRO_OAUTH_CLIENT_SECRET`, which [`OAuthConfig::from_env`] only reaches once
/// `YORISHIRO_OAUTH_ISSUER_URL` is already known to be set.
fn require_non_empty_env(key: &str) -> String {
    require_non_empty(key, crate::services::non_empty_env(key).as_deref())
}

/// The pure fold `require_non_empty_env` wraps around [`crate::services::non_empty_env`]'s
/// output, split out (and `pub`, re-exported from `oauth::mod`) so
/// `crates/yorishiro-hosted/tests/` can exercise every case (unset, set-but-empty, set)
/// without mutating the process environment. The empty case is the one that matters: `raw` being
/// `Some("")` must be rejected exactly like unset, because `client_secret` is also the HMAC key
/// for the CSRF `state` token. The `filter` below is redundant given `non_empty_env` already
/// excludes empty strings at the real call site: it's kept so `Some("")` remains a meaningful,
/// directly assertable input for the tests.
pub fn require_non_empty(key: &str, raw: Option<&str>) -> String {
    match raw.filter(|s| !s.is_empty()) {
        Some(value) => value.to_string(),
        None => panic!("{key} must be set to a non-empty value when YORISHIRO_OAUTH_ISSUER_URL is"),
    }
}

/// `{bind}/auth/oauth/callback`, per the design doc. `YORISHIRO_BIND` defaults to
/// `0.0.0.0:8080` (see `main`), which is very rarely the host a browser can actually reach:
/// most real deployments sit behind a reverse proxy on a public hostname, so they're expected to
/// set `YORISHIRO_OAUTH_REDIRECT_URI` explicitly. This default only covers same-host/local
/// testing, replacing an all-interfaces bind address with `localhost` since browsers can't dial
/// `0.0.0.0` (or `::`) directly.
fn default_redirect_uri() -> String {
    let bind = crate::services::bind_addr_from_env();
    format!(
        "http://{}/auth/oauth/callback",
        rewrite_unspecified_host(&bind)
    )
}

/// Rewrites `host:port` to `localhost:port` when the host is an all-interfaces bind address
/// (`0.0.0.0` or `::`, i.e. [`std::net::IpAddr::is_unspecified`]), leaving everything else
/// (including a real IP that merely *contains* the substring `"0.0.0.0"`, like `10.0.0.0`) as
/// given. Parses the whole string as a [`std::net::SocketAddr`] rather than doing a substring
/// replace, which corrupted addresses like `10.0.0.0:8081` into `1localhost:8081` (the earlier
/// bug this function replaced). A `bind` that isn't a valid `SocketAddr` at all (a hostname, or
/// an unbracketed IPv6 literal) is passed through unchanged: it's either already a real,
/// browser-reachable host, or malformed enough that guessing at a rewrite would only make things
/// worse.
///
/// `pub` (and re-exported from `oauth::mod`) purely so `crates/yorishiro-hosted/tests/` can
/// exercise its full input matrix as a table test without mutating process environment
/// variables: this is pure string logic with no side effects, not part of the OAuth request
/// flow itself.
pub fn rewrite_unspecified_host(bind: &str) -> String {
    match bind.parse::<std::net::SocketAddr>() {
        Ok(addr) if addr.ip().is_unspecified() => format!("localhost:{}", addr.port()),
        _ => bind.to_string(),
    }
}

#[cfg(test)]
#[path = "../../../tests/services/oauth/config.rs"]
mod tests;
