use reqwest::Client;
use std::time::Duration;
use yorishiro::YorishiroError;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use yorishiro::edition::ee::services::oauth::discovery::*;

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
    let (url, task) =
        start_server("HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json")
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

    let (url, task) =
        start_server("HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json")
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

    let (url, task) =
        start_server("HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json")
            .await;
    let error = http.fetch_jwks(&url).await.unwrap_err();
    task.await.expect("invalid JWKS server must finish");
    assert_decode_error(&error, &url);
}

#[tokio::test]
async fn accepted_redirect_status_error_names_the_original_endpoint() {
    let (final_url, final_task) =
        start_server("HTTP/1.1 418 I'm a teapot\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
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
