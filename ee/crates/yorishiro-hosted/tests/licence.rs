use yorishiro_hosted::services::licence::{LicenceClaims, LicenceState, licence_key_in, verify};

const TEST_PUBLIC_KEY: &[u8] = include_bytes!("keys/test-public.pem");
const TEST_PRIVATE_KEY_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/keys/test-private.pem");

fn sign(claims: &LicenceClaims) -> String {
    use std::process::{Command, Stdio};
    use std::io::Write;

    let header = base64_url(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = base64_url(serde_json::to_string(claims).unwrap().as_bytes());
    let signing_input = format!("{header}.{payload}");

    let mut child = Command::new("openssl")
        .args(["dgst", "-sha256", "-sign", TEST_PRIVATE_KEY_PATH])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("openssl must be on PATH to sign the test JWT");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(signing_input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "openssl sign failed: {output:?}");

    format!("{signing_input}.{}", base64_url(&output.stdout))
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[test]
fn verify_accepts_a_correctly_signed_unexpired_token() {
    let claims = LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() + 3600,
    };
    let token = sign(&claims);
    let verified = verify(&token, TEST_PUBLIC_KEY).expect("a correctly signed token must verify");
    assert_eq!(verified.plan, "enterprise");
}

#[test]
fn verify_rejects_a_token_signed_by_a_different_key() {
    let claims = LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() + 3600,
    };
    let token = sign(&claims);
    // The compiled-in production public key, not the test key that actually signed this token.
    let production_key = include_bytes!("../keys/licence-public.pem");
    assert!(verify(&token, production_key).is_err());
}

#[test]
fn is_active_at_treats_exp_as_exclusive_of_the_expiry_instant() {
    let state = LicenceState::licensed(LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: 1000,
    });
    assert!(state.is_active_at(999));
    assert!(!state.is_active_at(1000));
    assert!(!state.is_active_at(1001));
}

#[test]
fn require_active_is_ok_only_while_licensed_and_unexpired() {
    let unlicensed = LicenceState::default();
    assert!(unlicensed.require_active().is_err());

    let licensed = LicenceState::licensed(LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() + 3600,
    });
    assert!(licensed.require_active().is_ok());
}

#[test]
fn licence_key_in_reads_the_key_and_ignores_unrelated_yaml_fields() {
    let yaml = "server:\n  port: 5150\nlicense_key: from-file\n";
    assert_eq!(licence_key_in(yaml), Some("from-file".to_string()));
}

#[test]
fn licence_key_in_treats_an_empty_string_as_absent() {
    let yaml = "license_key: \"\"\n";
    assert_eq!(licence_key_in(yaml), None);
}

#[test]
fn licence_key_in_returns_none_when_the_key_is_missing_entirely() {
    assert_eq!(licence_key_in("server:\n  port: 5150\n"), None);
}
