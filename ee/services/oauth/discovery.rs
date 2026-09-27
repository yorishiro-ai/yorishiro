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
fn redirect_policy() -> reqwest::redirect::Policy {
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
    let parsed = url::Url::parse(url).internal()?;
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
pub(super) trait OAuthHttp: Send + Sync {
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
pub(super) struct DiscoveryDocument {
    pub(super) authorization_endpoint: String,
    pub(super) token_endpoint: String,
    pub(super) jwks_uri: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct TokenResponse {
    pub(super) id_token: String,
}

struct ReqwestOAuthHttp {
    client: Client,
}

impl ReqwestOAuthHttp {
    fn new() -> Self {
        Self {
            client: http_client(),
        }
    }

    #[cfg(test)]
    fn with_client(client: Client) -> Self {
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    async fn start_server(response: &str) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("test listener must bind");
        let address = listener.local_addr().expect("test listener has an address");
        let response = response.to_string();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("test request must arrive");
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let read = socket
                    .read(&mut buffer)
                    .await
                    .expect("test request must be readable");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let header_end = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .expect("request headers must be present")
                        + 4;
                    let content_length = String::from_utf8_lossy(&request[..header_end])
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("Content-Length:")
                                .or_else(|| line.strip_prefix("content-length:"))
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + content_length {
                        break;
                    }
                }
            }
            socket
                .write_all(response.as_bytes())
                .await
                .expect("test response must be written");
            request
        });
        (format!("http://{address}"), task)
    }

    fn client(timeout: Duration) -> Client {
        Client::builder()
            .timeout(timeout)
            .redirect(redirect_policy())
            .build()
            .expect("test client configuration must be valid")
    }

    #[tokio::test]
    async fn production_transport_fetches_discovery_and_jwks_and_exchanges_code() {
        let (issuer, discovery_task) = start_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 132\r\nConnection: close\r\n\r\n{\"authorization_endpoint\":\"http://127.0.0.1/authorize\",\"token_endpoint\":\"http://127.0.0.1/token\",\"jwks_uri\":\"http://127.0.0.1/jwks\"}",
        )
        .await;
        let http = ReqwestOAuthHttp::with_client(client(Duration::from_secs(1)));
        let discovery = http
            .fetch_discovery_document(&issuer)
            .await
            .expect("discovery response must parse");
        discovery_task.await.expect("discovery server must finish");
        assert_eq!(
            discovery.authorization_endpoint,
            "http://127.0.0.1/authorize"
        );

        let (token_endpoint, token_task) = start_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 28\r\nConnection: close\r\n\r\n{\"id_token\":\"test-id-token\"}",
        )
        .await;
        let tokens = http
            .exchange_code_for_tokens(
                &format!("{token_endpoint}/token"),
                "client-id",
                "client-secret",
                "code",
                "http://localhost/callback",
                "verifier",
            )
            .await
            .expect("token response must parse");
        let token_request = token_task.await.expect("token server must finish");
        assert_eq!(tokens.id_token, "test-id-token");
        let token_request = String::from_utf8(token_request).expect("token request is HTTP text");
        assert!(token_request.starts_with("POST /token HTTP/1.1\r\n"));
        let token_body = token_request
            .split_once("\r\n\r\n")
            .expect("token request must have a body")
            .1;
        for parameter in [
            "grant_type=authorization_code",
            "code=code",
            "redirect_uri=http%3A%2F%2Flocalhost%2Fcallback",
            "client_id=client-id",
            "client_secret=client-secret",
            "code_verifier=verifier",
        ] {
            assert!(
                token_body.contains(parameter),
                "missing form parameter: {parameter}"
            );
        }

        let (jwks_uri, jwks_task) = start_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"keys\":[]}",
        )
        .await;
        let jwks = http
            .fetch_jwks(&jwks_uri)
            .await
            .expect("JWKS response must parse");
        jwks_task.await.expect("JWKS server must finish");
        assert!(jwks.keys.is_empty());
    }

    #[tokio::test]
    async fn transport_failure_maps_to_internal_without_provider_secret() {
        let http = ReqwestOAuthHttp::with_client(client(Duration::from_millis(100)));
        let error = http
            .fetch_discovery_document("http://127.0.0.1:1")
            .await
            .unwrap_err();

        assert!(matches!(error, YorishiroError::Internal(_)));
        assert!(!error.to_string().contains("client-secret"));
    }

    #[tokio::test]
    async fn token_transport_failure_redacts_client_secret() {
        let http = ReqwestOAuthHttp::with_client(client(Duration::from_millis(100)));
        let error = http
            .exchange_code_for_tokens(
                "http://127.0.0.1:1/token",
                "client-id",
                "client-secret",
                "code",
                "http://localhost/callback",
                "verifier",
            )
            .await
            .unwrap_err();

        assert!(matches!(error, YorishiroError::Internal(_)));
        assert!(!error.to_string().contains("client-secret"));
    }

    #[tokio::test]
    async fn timeout_maps_to_internal() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("test listener must bind");
        let address = listener.local_addr().expect("test listener has an address");
        let task = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.expect("test request must arrive");
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let http = ReqwestOAuthHttp::with_client(client(Duration::from_millis(10)));
        let error = http
            .fetch_discovery_document(&format!("http://{address}"))
            .await
            .unwrap_err();

        task.await.expect("timeout server must finish");
        assert!(matches!(error, YorishiroError::Internal(_)));
    }

    #[tokio::test]
    async fn invalid_responses_and_provider_statuses_keep_existing_mapping() {
        let (url, task) = start_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json",
        )
        .await;
        let http = ReqwestOAuthHttp::with_client(client(Duration::from_secs(1)));
        let error = http.fetch_discovery_document(&url).await.unwrap_err();
        task.await.expect("invalid discovery server must finish");
        assert_decode_error(&error, &format!("{url}/.well-known/openid-configuration"));

        let (url, task) = start_server(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 20\r\nConnection: close\r\n\r\nprovider-secret-body",
        )
        .await;
        let error = http
            .exchange_code_for_tokens(
                &url,
                "client-id",
                "client-secret",
                "code",
                "http://localhost/callback",
                "verifier",
            )
            .await
            .unwrap_err();
        task.await.expect("token error server must finish");
        assert!(matches!(error, YorishiroError::Unauthenticated));
        assert!(!format!("{error:?}").contains("provider-secret-body"));

        let (url, task) = start_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json",
        )
        .await;
        let error = http
            .exchange_code_for_tokens(
                &url,
                "client-id",
                "client-secret",
                "code",
                "http://localhost/callback",
                "verifier",
            )
            .await
            .unwrap_err();
        task.await.expect("invalid token server must finish");
        assert_decode_error(&error, &url);

        let (url, task) = start_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json",
        )
        .await;
        let error = http.fetch_jwks(&url).await.unwrap_err();
        task.await.expect("invalid JWKS server must finish");
        assert_decode_error(&error, &url);
    }

    #[tokio::test]
    async fn accepted_redirect_status_error_names_the_original_endpoint() {
        let (final_url, final_task) = start_server(
            "HTTP/1.1 418 I'm a teapot\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let (redirect_url, redirect_task) = start_server(&format!(
            "HTTP/1.1 302 Found\r\nLocation: {final_url}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ))
        .await;
        let original_url = format!("{redirect_url}/start");
        let http = ReqwestOAuthHttp::with_client(client(Duration::from_secs(1)));

        let error = http.fetch_jwks(&original_url).await.unwrap_err();

        redirect_task.await.expect("redirect server must finish");
        final_task.await.expect("final server must finish");
        assert!(error.to_string().contains(&original_url));
        assert!(!error.to_string().contains(&format!("{final_url}/final")));
    }

    fn assert_decode_error(error: &YorishiroError, url: &str) {
        assert!(matches!(error, YorishiroError::Internal(_)));
        assert!(error.to_string().contains("error decoding response body"));
        assert!(format!("{error:?}").contains(url));
        assert!(!format!("{error:?}").contains("not-json"));
    }

    #[tokio::test]
    async fn rejects_non_https_public_urls_and_redirect_targets() {
        let http = ReqwestOAuthHttp::with_client(client(Duration::from_secs(1)));
        let error = http
            .fetch_jwks("http://idp.example.invalid/jwks")
            .await
            .unwrap_err();
        assert!(matches!(error, YorishiroError::Internal(_)));

        let (url, task) = start_server(
            "HTTP/1.1 302 Found\r\nLocation: http://idp.example.invalid/jwks\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let error = http.fetch_jwks(&url).await.unwrap_err();
        task.await.expect("redirect server must finish");
        assert!(matches!(error, YorishiroError::Internal(_)));
    }
}
