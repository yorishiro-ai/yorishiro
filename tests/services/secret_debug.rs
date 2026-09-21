use std::fmt::Debug;

use uuid::Uuid;
use yorishiro::ee::controllers::embedding::SetEmbeddingKeyRequest;
use yorishiro::ee::controllers::inference::SetLlmKeyRequest;
use yorishiro::ee::controllers::stripe::StripeConfig;
use yorishiro::ee::services::inference::InferenceConfig;
use yorishiro::ee::services::licence::{LicenceClaims, LicenceState};
use yorishiro::ee::services::oauth::ProvisionedLogin;
use yorishiro::ee::services::oauth::config::OAuthConfig;
use yorishiro::models::tenancy::MembershipRole;

fn rendered<T: Debug>(value: &T) -> String {
    format!("{value:?}")
}

#[test]
fn inference_config_debug_redacts_api_key() {
    let config = InferenceConfig {
        base_url: "https://provider.example/v1".into(),
        model: "model".into(),
        api_key: "do-not-render-inference-value".into(),
    };

    let output = rendered(&config);

    assert!(!output.contains("do-not-render-inference-value"));
    assert!(output.contains("provider.example"));
}

#[test]
fn oauth_config_debug_redacts_client_secret_and_state_signing_key() {
    let config = OAuthConfig {
        issuer_url: "https://issuer.example".into(),
        client_id: "client-id".into(),
        client_secret: "do-not-render-oauth-value".into(),
        redirect_uri: "https://app.example/callback".into(),
        state_signing_key: b"do-not-render-state-value".to_vec(),
    };

    let output = rendered(&config);

    assert!(!output.contains("do-not-render-oauth-value"));
    assert!(!output.contains("do-not-render-state-value"));
    assert!(output.contains("issuer.example"));
}

#[test]
fn stripe_config_debug_redacts_webhook_secret() {
    let config = StripeConfig {
        webhook_secret: Some("do-not-render-stripe-value".into()),
        price_mapping: Default::default(),
    };

    let output = rendered(&config);

    assert!(!output.contains("do-not-render-stripe-value"));
    assert!(output.contains("<redacted>"));
}

#[test]
fn licence_debug_redacts_subject_directly_and_when_nested() {
    let claims = LicenceClaims {
        sub: "licence-owner@example.invalid".into(),
        plan: "team".into(),
        exp: 1_900_000_000,
    };
    let direct = rendered(&claims);
    let nested = rendered(&LicenceState::licensed(claims));

    assert!(!direct.contains("licence-owner@example.invalid"));
    assert!(!nested.contains("licence-owner@example.invalid"));
    assert!(nested.contains("team"));
}

#[test]
fn provisioned_login_debug_redacts_email() {
    let login = ProvisionedLogin {
        user_id: Uuid::nil(),
        email: "oidc-user@example.invalid".into(),
        workspace_id: Uuid::nil(),
        role: MembershipRole::Member,
    };

    let output = rendered(&login);

    assert!(!output.contains("oidc-user@example.invalid"));
    assert!(output.contains("workspace_id"));
}

#[test]
fn public_key_requests_debug_redacts_api_keys() {
    let embedding = SetEmbeddingKeyRequest {
        base_url: "https://provider.example".into(),
        model: "model".into(),
        api_key: "do-not-render-embedding-value".into(),
        dimensions: 768,
        send_dimensions_param: false,
    };
    let inference = SetLlmKeyRequest {
        base_url: "https://provider.example".into(),
        model: "model".into(),
        api_key: "do-not-render-llm-value".into(),
    };

    let embedding_output = rendered(&embedding);
    let inference_output = rendered(&inference);

    assert!(!embedding_output.contains("do-not-render-embedding-value"));
    assert!(!inference_output.contains("do-not-render-llm-value"));
    assert!(embedding_output.contains("provider.example"));
    assert!(inference_output.contains("provider.example"));
}
