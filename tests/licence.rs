use yorishiro::ee::services::licence::{LicenceClaims, LicenceState, licence_key_in, verify};

/// A keypair generated fresh per test process rather than checked into the repository: it signs only throwaway JWTs this suite mints and verifies itself, so committing it would be a private key in source control for no reason a scanner can tell apart from a real one.
struct TestKeypair {
    private_key_path: std::path::PathBuf,
    public_key_pem: Vec<u8>,
    _dir: tempfile::TempDir,
}

fn test_keypair() -> TestKeypair {
    use std::process::Command;

    let dir = tempfile::tempdir().expect("tempdir for the test keypair");
    let private_key_path = dir.path().join("private.pem");
    let public_key_path = dir.path().join("public.pem");

    let status = Command::new("openssl")
        .args(["genrsa", "-out", private_key_path.to_str().unwrap(), "2048"])
        .status()
        .expect("openssl must be on PATH to generate the test keypair");
    assert!(status.success(), "openssl genrsa failed");

    let status = Command::new("openssl")
        .args([
            "rsa",
            "-in",
            private_key_path.to_str().unwrap(),
            "-pubout",
            "-out",
            public_key_path.to_str().unwrap(),
        ])
        .status()
        .expect("openssl must be on PATH to derive the test public key");
    assert!(status.success(), "openssl rsa -pubout failed");

    let public_key_pem = std::fs::read(&public_key_path).expect("reading the derived public key");

    TestKeypair {
        private_key_path,
        public_key_pem,
        _dir: dir,
    }
}

fn sign(private_key_path: &std::path::Path, claims: &LicenceClaims) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let header = base64_url(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = base64_url(serde_json::to_string(claims).unwrap().as_bytes());
    let signing_input = format!("{header}.{payload}");

    let mut child = Command::new("openssl")
        .args([
            "dgst",
            "-sha256",
            "-sign",
            private_key_path.to_str().unwrap(),
        ])
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
    let keypair = test_keypair();
    let claims = LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() + 3600,
    };
    let token = sign(&keypair.private_key_path, &claims);
    let verified =
        verify(&token, &keypair.public_key_pem).expect("a correctly signed token must verify");
    assert_eq!(verified.plan, "enterprise");
}

#[test]
fn verify_rejects_a_token_signed_by_a_different_key() {
    let keypair = test_keypair();
    let claims = LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() + 3600,
    };
    let token = sign(&keypair.private_key_path, &claims);
    // The compiled-in production public key, not the test key that actually signed this token.
    let production_key = include_bytes!("../ee/keys/licence-public.pem");
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

/// A state carrying no claims is inactive at every instant, which is what an absent or
/// unverifiable key resolves to. The expiry boundary is covered above; this covers the other way
/// a licence fails to be active.
#[test]
fn a_state_with_no_claims_is_never_active() {
    let unlicensed = LicenceState::default();
    assert!(!unlicensed.is_active_at(0));
    assert!(!unlicensed.is_active_at(chrono::Utc::now().timestamp()));

    let licensed = LicenceState::licensed(LicenceClaims {
        sub: "test-customer".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() + 3600,
    });
    assert!(licensed.is_active());
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
