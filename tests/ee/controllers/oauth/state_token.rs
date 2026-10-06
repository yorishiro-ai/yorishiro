use yorishiro::edition::ee::controllers::hmac_sign;
use yorishiro::edition::ee::controllers::oauth::state_token::*;

const KEY: &[u8] = b"a signing key";

#[test]
fn issued_state_verifies_and_the_csrf_hash_matches_the_cookie() {
    let issued = issue_at(KEY, 1_700_000_000);
    let verified =
        verify_at(KEY, &issued.state, 1_700_000_000).expect("a freshly issued state must verify");
    assert_eq!(
        verified.csrf_hash,
        hash_csrf_cookie(&issued.csrf_cookie_value)
    );
}

#[test]
fn verify_rejects_a_wrong_signing_key() {
    let issued = issue_at(KEY, 1_700_000_000);
    assert!(verify_at(b"a different key", &issued.state, 1_700_000_000).is_none());
}

#[test]
fn verify_rejects_a_tampered_payload() {
    let issued = issue_at(KEY, 1_700_000_000);
    // Flip the last character of the payload without re-signing.
    let mut parts: Vec<&str> = issued.state.split('.').collect();
    let last_payload_part = parts[2].to_string();
    let mut tampered_part = last_payload_part.clone();
    tampered_part.push('x');
    parts[2] = &tampered_part;
    let tampered = parts.join(".");

    assert!(verify_at(KEY, &tampered, 1_700_000_000).is_none());
}

#[test]
fn verify_rejects_a_malformed_state() {
    assert!(verify(KEY, "not.enough.parts").is_none());
    assert!(verify(KEY, "").is_none());
}

#[test]
fn verify_rejects_an_expired_state() {
    let now = 1_700_000_000;
    let issued_at = now - STATE_TTL_SECS - 1;
    let csrf_hash = "deadbeef";
    let pkce_verifier = "verifier";
    let payload = format!("{issued_at}.{csrf_hash}.{pkce_verifier}");
    let signature = hmac_sign::sign(KEY, payload.as_bytes());
    let state = format!("{payload}.{signature}");

    assert!(verify_at(KEY, &state, now).is_none());
}

#[test]
fn verify_accepts_the_expiry_and_future_skew_boundaries() {
    let now = 1_700_000_000;
    for issued_at in [now - STATE_TTL_SECS, now + 5] {
        let csrf_hash = "deadbeef";
        let pkce_verifier = "verifier";
        let payload = format!("{issued_at}.{csrf_hash}.{pkce_verifier}");
        let signature = hmac_sign::sign(KEY, payload.as_bytes());
        let state = format!("{payload}.{signature}");

        assert!(verify_at(KEY, &state, now).is_some());
    }
}

#[test]
fn verify_rejects_a_state_outside_the_future_skew_boundary() {
    let now = 1_700_000_000;
    let issued_at = now + 6;
    let csrf_hash = "deadbeef";
    let pkce_verifier = "verifier";
    let payload = format!("{issued_at}.{csrf_hash}.{pkce_verifier}");
    let signature = hmac_sign::sign(KEY, payload.as_bytes());
    let state = format!("{payload}.{signature}");

    assert!(verify_at(KEY, &state, now).is_none());
}

#[test]
fn verify_handles_extreme_timestamps_without_overflow() {
    for (issued_at, now) in [(i64::MIN, i64::MAX), (i64::MAX, i64::MIN)] {
        let payload = format!("{issued_at}.deadbeef.verifier");
        let signature = hmac_sign::sign(KEY, payload.as_bytes());
        let state = format!("{payload}.{signature}");

        assert!(verify_at(KEY, &state, now).is_none());
    }
}

#[test]
fn production_wrappers_issue_and_verify_a_state() {
    let issued = issue(KEY);
    let verified = verify(KEY, &issued.state).expect("a production state must verify");
    assert_eq!(
        verified.csrf_hash,
        hash_csrf_cookie(&issued.csrf_cookie_value)
    );
}
