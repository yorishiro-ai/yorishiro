use super::{LicenceClaims, LicenceState, verify};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

/// A test-only keypair, committed under `tests/fixtures/`. Deliberately not the key the binary
/// embeds: signing with this one and verifying against the real public key must fail, which is
/// what `a_key_signed_by_another_issuer_is_refused` asserts.
const TEST_PRIVATE_KEY: &[u8] = include_bytes!("../fixtures/test-licence-key.pem");
const TEST_PUBLIC_KEY: &[u8] = include_bytes!("../fixtures/test-licence-public.pem");
const REAL_PUBLIC_KEY: &[u8] = include_bytes!("../../keys/licence-public.pem");

fn sign(claims: &LicenceClaims, private_key: &[u8]) -> String {
    let key = EncodingKey::from_rsa_pem(private_key).expect("test key is a valid RSA PEM");
    encode(&Header::new(Algorithm::RS256), claims, &key).expect("signing a test licence")
}

fn claims(exp: i64) -> LicenceClaims {
    LicenceClaims {
        sub: "acme-corp".into(),
        plan: "enterprise".into(),
        exp,
    }
}

fn far_future() -> i64 {
    chrono::Utc::now().timestamp() + 60 * 60 * 24 * 365
}

#[test]
fn a_validly_signed_key_verifies_and_carries_its_claims() {
    let token = sign(&claims(far_future()), TEST_PRIVATE_KEY);

    let verified = verify(&token, TEST_PUBLIC_KEY).expect("a validly signed key verifies");

    assert_eq!(verified.sub, "acme-corp");
    assert_eq!(verified.plan, "enterprise");
}

#[test]
fn a_key_signed_by_another_issuer_is_refused() {
    let token = sign(&claims(far_future()), TEST_PRIVATE_KEY);

    // Signed with the test key, checked against the one the binary ships with.
    assert!(verify(&token, REAL_PUBLIC_KEY).is_err());
}

/// Well past `Validation`'s default 60s leeway, which exists for clock skew and would otherwise
/// accept a key that expired seconds ago. The leeway is left at its default: a minute either way
/// does not matter for a licence measured in months, and `is_active_at` is the gate that decides
/// during a run anyway.
#[test]
fn an_expired_key_does_not_verify() {
    let an_hour_ago = chrono::Utc::now().timestamp() - 60 * 60;
    let token = sign(&claims(an_hour_ago), TEST_PRIVATE_KEY);

    assert!(verify(&token, TEST_PUBLIC_KEY).is_err());
}

/// The leeway is real, and stating it here keeps the previous test honest about *why* it uses an
/// hour rather than a second.
#[test]
fn a_key_expiring_within_the_clock_skew_leeway_still_verifies() {
    let just_expired = chrono::Utc::now().timestamp() - 5;
    let token = sign(&claims(just_expired), TEST_PRIVATE_KEY);

    assert!(verify(&token, TEST_PUBLIC_KEY).is_ok());
    // But the gate that runs on every request is not fooled by it.
    let state = LicenceState::licensed(claims(just_expired));
    assert!(!state.is_active());
}

#[test]
fn a_tampered_payload_does_not_verify() {
    let token = sign(&claims(far_future()), TEST_PRIVATE_KEY);
    // Flip a character in the payload segment, leaving the signature as it was.
    let mut parts: Vec<&str> = token.split('.').collect();
    let payload = parts[1].to_string();
    let tampered = format!("{}X{}", &payload[..payload.len() - 1], "");
    parts[1] = &tampered;
    let tampered_token = parts.join(".");

    assert!(verify(&tampered_token, TEST_PUBLIC_KEY).is_err());
}

#[test]
fn garbage_is_refused_rather_than_panicking() {
    for bad in ["", "not-a-jwt", "a.b.c", "...."] {
        assert!(verify(bad, TEST_PUBLIC_KEY).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn no_licence_means_paid_features_are_off() {
    let state = LicenceState::default();

    assert!(!state.is_active());
    assert!(state.require_active().is_err());
}

#[test]
fn a_licence_is_active_until_its_expiry_and_not_after() {
    let state = LicenceState::licensed(claims(1_000));

    // Expiry is compared at the moment of the check, not frozen when the process started, so a
    // key that lapses mid-run stops working without a restart.
    assert!(state.is_active_at(999));
    assert!(!state.is_active_at(1_000), "expiry itself is not active");
    assert!(!state.is_active_at(1_001));
}

#[test]
fn an_active_licence_permits_a_gated_feature() {
    let state = LicenceState::licensed(claims(far_future()));

    assert!(state.is_active());
    assert!(state.require_active().is_ok());
}

/// The licence may be configured in `config.yml` rather than the environment, and this is the
/// half that reads it. `deny_unknown_fields` is deliberately absent: the file it parses is the
/// server's whole configuration, so every key that belongs to another struct has to pass
/// through rather than fail the parse.
#[test]
fn the_licence_key_is_read_from_a_config_file() {
    use crate::services::licence::licence_key_in;

    assert_eq!(
        licence_key_in("database_url: postgres://x\nlicense_key: a-token\n"),
        Some("a-token".into()),
        "a key alongside unrelated settings"
    );
    assert_eq!(
        licence_key_in("database_url: postgres://x\n"),
        None,
        "absent"
    );
    assert_eq!(
        licence_key_in("license_key: \"\"\n"),
        None,
        "empty is absent"
    );
    assert_eq!(
        licence_key_in(": : not yaml\n"),
        None,
        "unparseable is absent, not a panic"
    );
}

/// The environment wins over the config file, and **set-but-empty is the environment winning**,
/// not an absence that lets the file through.
///
/// Without that, `YORISHIRO_LICENSE_KEY=` could not turn off a licence configured in the file,
/// which is the opposite of what "the environment takes precedence" means. Every other setting
/// behaves this way: the shared loader skips the file whenever the variable exists at all.
#[test]
fn an_empty_environment_key_does_not_fall_through_to_the_file() {
    use crate::services::licence::resolve_licence_key;

    let from_file = || Some("from-file".to_string());

    assert_eq!(
        resolve_licence_key(Some("from-env".into()), from_file),
        Some("from-env".into()),
        "a value in the environment wins"
    );
    assert_eq!(
        resolve_licence_key(Some(String::new()), from_file),
        None,
        "empty means no licence, and the file is not consulted"
    );
    assert_eq!(
        resolve_licence_key(None, from_file),
        Some("from-file".into()),
        "absent is what lets the file through"
    );
    assert_eq!(
        resolve_licence_key(None, || None),
        None,
        "neither source configured"
    );
}
