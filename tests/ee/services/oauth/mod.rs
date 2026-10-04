mod discovery;

use std::sync::Arc;
use std::sync::Mutex;
use yorishiro::YorishiroError;
use yorishiro::ee::controllers::oauth::state_token;
use yorishiro::ee::data::oauth::OAuthConfig;

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;

use yorishiro::ee::services::oauth::*;

struct FakeOAuthHttp {
    calls: Mutex<Vec<&'static str>>,
}

#[async_trait]
impl yorishiro::ee::services::oauth::discovery::OAuthHttp for FakeOAuthHttp {
    async fn fetch_discovery_document(
        &self,
        _issuer_url: &str,
    ) -> Result<yorishiro::ee::services::oauth::discovery::DiscoveryDocument, YorishiroError> {
        self.calls.lock().unwrap().push("discovery");
        Ok(
            yorishiro::ee::services::oauth::discovery::DiscoveryDocument {
                authorization_endpoint: "https://idp.example/authorize".into(),
                token_endpoint: "https://idp.example/token".into(),
                jwks_uri: "https://idp.example/jwks".into(),
            },
        )
    }

    async fn exchange_code_for_tokens(
        &self,
        _token_endpoint: &str,
        _client_id: &str,
        _client_secret: &str,
        _code: &str,
        _redirect_uri: &str,
        _pkce_verifier: &str,
    ) -> Result<yorishiro::ee::services::oauth::discovery::TokenResponse, YorishiroError> {
        self.calls.lock().unwrap().push("token");
        Ok(yorishiro::ee::services::oauth::discovery::TokenResponse {
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
