mod discovery;

use serde_json::json;
use yorishiro::YorishiroError;
use yorishiro::edition::ee::controllers::oauth::state_token;
use yorishiro::edition::ee::data::oauth::OAuthConfig;
use yorishiro::edition::ee::services::oauth::{build_authorize_redirect, handle_callback};

fn config(issuer_url: String) -> OAuthConfig {
    OAuthConfig {
        issuer_url,
        client_id: "client-id".into(),
        client_secret: "client-secret".into(),
        redirect_uri: "http://localhost:5150/auth/oauth/callback".into(),
        state_signing_key: b"client-secret".to_vec(),
    }
}

async fn json_server(
    body: serde_json::Value,
) -> (
    String,
    crate::http_stub::HttpStub,
    tokio::sync::oneshot::Receiver<Vec<u8>>,
) {
    let body = body.to_string();
    discovery::start_server(&format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    ))
    .await
}

#[tokio::test]
async fn authorize_uses_production_discovery() {
    let (issuer, _server_stub, server) = json_server(json!({
        "authorization_endpoint": "https://idp.example/authorize",
        "token_endpoint": "https://idp.example/token",
        "jwks_uri": "https://idp.example/jwks",
    }))
    .await;
    let redirect = build_authorize_redirect(&config(issuer)).await.unwrap();
    assert!(redirect.url.starts_with("https://idp.example/authorize?"));
    assert!(redirect.url.contains("code_challenge_method=S256"));
    assert!(!redirect.csrf_cookie_value.is_empty());
    let request = server.await.unwrap();
    assert!(
        String::from_utf8_lossy(&request).starts_with("GET /.well-known/openid-configuration ")
    );
}

#[tokio::test]
async fn callback_uses_production_discovery_exchange_and_jwks_before_verifying_token() {
    let (token_endpoint, _token_server_stub, token_server) =
        json_server(json!({"id_token": "not-a-jwt"})).await;
    let (jwks_uri, _jwks_server_stub, jwks_server) = json_server(json!({"keys": []})).await;
    let (issuer, _discovery_server_stub, discovery_server) = json_server(json!({
        "authorization_endpoint": "https://idp.example/authorize",
        "token_endpoint": token_endpoint,
        "jwks_uri": jwks_uri,
    }))
    .await;
    let issued = state_token::issue(b"client-secret");
    let result = handle_callback(
        &config(issuer),
        "authorization-code",
        &issued.state,
        Some(&issued.csrf_cookie_value),
    )
    .await;
    assert!(matches!(result, Err(YorishiroError::Unauthenticated)));
    discovery_server.await.unwrap();
    jwks_server.await.unwrap();
    let request = token_server.await.unwrap();
    let request = String::from_utf8_lossy(&request);
    assert!(request.starts_with("POST / "));
    assert!(request.contains("code=authorization-code"));
    assert!(request.contains("grant_type=authorization_code"));
    assert!(request.contains("code_verifier="));
}
