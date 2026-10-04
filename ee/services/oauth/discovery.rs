//! OIDC discovery, authorization-code exchange, and JWKS retrieval.
//!
//! The private transport seam covers exactly the provider operations used by the OAuth flow.
//! URL policy, timeout, response parsing, and error redaction stay in the production adapter.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use reqwest::Client;
use serde::Deserialize;

use crate::YorishiroError;
use crate::error::ResultExt;

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
pub fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if !is_https_or_loopback(attempt.url()) {
            return attempt.error("refusing to follow a redirect to a non-https, non-loopback URL");
        }
        reqwest::redirect::Policy::default().redirect(attempt)
    })
}

fn http_client() -> Client {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(redirect_policy())
        .build()
        .expect("reqwest client configuration is static and always valid")
}

/// Applies `is_https_or_loopback` to the initial request URL itself: the same rule `redirect_policy` enforces for every redirect hop.
fn require_https_or_loopback(url: &str) -> Result<(), YorishiroError> {
    let parsed = reqwest::Url::parse(url).internal()?;
    if is_https_or_loopback(&parsed) {
        return Ok(());
    }
    Err(YorishiroError::Internal(anyhow::anyhow!(
        "refusing a plaintext request to '{url}': OIDC endpoints must use https:// (loopback \
         hosts are exempt for local development)"
    )))
}

/// The OAuth/OIDC HTTP boundary used by the flow.
///
/// It is private to the OAuth service and names only the three provider operations that need deterministic substitution.
#[async_trait]
pub trait OAuthHttp: Send + Sync {
    async fn fetch_discovery_document(
        &self,
        issuer_url: &str,
    ) -> Result<DiscoveryDocument, YorishiroError>;

    async fn exchange_code_for_tokens(
        &self,
        token_endpoint: &str,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        pkce_verifier: &str,
    ) -> Result<TokenResponse, YorishiroError>;

    async fn fetch_jwks(&self, jwks_uri: &str) -> Result<JwkSet, YorishiroError>;
}

/// The subset of an OIDC discovery document (`{issuer}/.well-known/openid-configuration`) this crate reads.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveryDocument {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub id_token: String,
}

pub struct ReqwestOAuthHttp {
    client: Client,
}

impl ReqwestOAuthHttp {
    fn new() -> Self {
        Self {
            client: http_client(),
        }
    }

    /// Builds the adapter around a caller-supplied client, for callers that need their own timeouts or transport.
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, YorishiroError> {
        require_https_or_loopback(url)?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| request_error(err, None))?;
        response_body(response, url, None).await
    }
}

pub(super) fn production() -> Arc<dyn OAuthHttp> {
    Arc::new(ReqwestOAuthHttp::new())
}

#[async_trait]
impl OAuthHttp for ReqwestOAuthHttp {
    async fn fetch_discovery_document(
        &self,
        issuer_url: &str,
    ) -> Result<DiscoveryDocument, YorishiroError> {
        self.get_json(&format!("{issuer_url}/.well-known/openid-configuration"))
            .await
    }

    async fn exchange_code_for_tokens(
        &self,
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

        let response = self
            .client
            .post(token_endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&params)
            .send()
            .await
            .map_err(|err| request_error(err, Some(client_secret)))?;

        if !response.status().is_success() {
            let status = response.status();
            tracing::warn!(%status, "OAuth token exchange rejected by provider");
            return Err(YorishiroError::Unauthenticated);
        }

        response_body(response, token_endpoint, Some(client_secret)).await
    }

    async fn fetch_jwks(&self, jwks_uri: &str) -> Result<JwkSet, YorishiroError> {
        self.get_json(jwks_uri).await
    }
}

async fn response_body<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    request_url: &str,
    secret: Option<&str>,
) -> Result<T, YorishiroError> {
    if !response.status().is_success() {
        return Err(YorishiroError::Internal(anyhow::anyhow!(
            "request to '{}' failed with status {}",
            request_url,
            response.status()
        )));
    }
    response
        .json::<T>()
        .await
        .map_err(|err| request_error(err, secret))
}

fn request_error(error: reqwest::Error, secret: Option<&str>) -> YorishiroError {
    let Some(secret) = secret.filter(|secret| !secret.is_empty()) else {
        return internal_reqwest(error);
    };
    let message = error.to_string();
    if !message.contains(secret) {
        return internal_reqwest(error);
    }
    YorishiroError::Internal(anyhow::anyhow!(message.replace(secret, "[redacted]")))
}

fn internal_reqwest(error: reqwest::Error) -> YorishiroError {
    match Err::<(), _>(error).internal() {
        Ok(()) => unreachable!("an error result cannot convert to Ok"),
        Err(error) => error,
    }
}
