//! OIDC discovery and token exchange.
//!
//! This hand-rolls just the three HTTP calls the flow needs: authorize redirect, code exchange, ID token parse, against whatever the provider's own discovery document says.

use std::time::Duration;

use jsonwebtoken::jwk::JwkSet;
use serde::Deserialize;
use yorishiro_core::YorishiroError;
use yorishiro_core::error::ResultExt;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// `true` for a host that only ever means "this machine": the one case an `http://` OIDC endpoint is legitimate.
/// Every other host must be reached over `https://`, see `redirect_policy`.
fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `true` for a URL this crate is willing to send an OIDC discovery/JWKS/token request to: `https://`, or plain `http://` if the host is loopback (see `is_loopback_host`).
fn is_https_or_loopback(url: &reqwest::Url) -> bool {
    url.scheme() == "https" || url.host_str().is_some_and(is_loopback_host)
}

/// Rejects a redirect whose target is not itself `https://`-or-loopback, regardless of the scheme the request that is being redirected started on: `reqwest`'s default policy follows redirects across schemes without restriction, which would let a compromised or misconfigured hop silently redirect an OIDC request to a plaintext target.
/// Delegates every accepted target to `Policy::default()`'s own `redirect`: a custom policy does not inherit the default policy's 10-hop limit and loop detection automatically.
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if !is_https_or_loopback(attempt.url()) {
            return attempt.error("refusing to follow a redirect to a non-https, non-loopback URL");
        }
        reqwest::redirect::Policy::default().redirect(attempt)
    })
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(redirect_policy())
        .build()
        .expect("reqwest client configuration is static and always valid")
}

/// Applies `is_https_or_loopback` to the initial request URL itself: the same rule `redirect_policy` enforces for every redirect hop.
fn require_https_or_loopback(url: &str) -> Result<(), YorishiroError> {
    let parsed = url::Url::parse(url).internal()?;
    if is_https_or_loopback(&parsed) {
        return Ok(());
    }
    Err(YorishiroError::Internal(anyhow::anyhow!(
        "refusing a plaintext request to '{url}': OIDC endpoints must use https:// (loopback \
         hosts are exempt for local development)"
    )))
}

/// The subset of an OIDC discovery document (`{issuer}/.well-known/openid-configuration`) this crate reads.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveryDocument {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// `GET`s `url` and parses the response body as JSON, failing with a `YorishiroError::Internal` that names `url` and the rejecting status if the response is not a 2xx.
async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, YorishiroError> {
    require_https_or_loopback(url)?;
    let response = http_client().get(url).send().await.internal()?;
    if !response.status().is_success() {
        return Err(YorishiroError::Internal(anyhow::anyhow!(
            "request to '{url}' failed with status {}",
            response.status()
        )));
    }
    response.json().await.internal()
}

/// Fetches and parses the issuer's discovery document.
/// Not cached: runs once per authorize/callback request so a provider rotating its endpoint or key set is never served stale, acceptable since login is not a hot path.
pub async fn fetch_discovery_document(
    issuer_url: &str,
) -> Result<DiscoveryDocument, YorishiroError> {
    get_json(&format!("{issuer_url}/.well-known/openid-configuration")).await
}

/// Fetches the provider's JSON Web Key Set from the `jwks_uri` named in its discovery document.
/// Used to verify the ID token's signature.
pub async fn fetch_jwks(jwks_uri: &str) -> Result<JwkSet, YorishiroError> {
    get_json(jwks_uri).await
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub id_token: String,
}

/// Exchanges an authorization code for tokens at the provider's token endpoint (the `authorization_code` grant, RFC 6749 §4.1.3), including the PKCE code verifier (RFC 7636) generated when the flow started.
pub async fn exchange_code_for_tokens(
    token_endpoint: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    pkce_verifier: &str,
) -> Result<TokenResponse, YorishiroError> {
    require_https_or_loopback(token_endpoint)?;
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code_verifier", pkce_verifier),
    ];

    let response = http_client()
        .post(token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&params)
        .send()
        .await
        .internal()?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, body, "OAuth token exchange rejected by provider");
        return Err(YorishiroError::Unauthenticated);
    }

    response.json().await.internal()
}
