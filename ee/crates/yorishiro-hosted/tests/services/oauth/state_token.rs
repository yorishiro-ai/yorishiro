use super::*;

#[test]
fn round_trips_a_freshly_issued_state() {
    let key = b"test-signing-key";
    let issued = issue(key);
    let verified = verify(key, &issued.state).expect("valid state should verify");
    // The recovered verifier must hash to the challenge that was sent to the provider at
    // authorize time: that's the actual PKCE property this round trip needs to preserve.
    let recomputed_challenge =
        URL_SAFE_NO_PAD.encode(Sha256::digest(verified.pkce_verifier.as_bytes()));
    assert_eq!(recomputed_challenge, issued.pkce_challenge);
    // The CSRF hash embedded in `state` must match the cookie value `issue` handed back.
    assert_eq!(
        verified.csrf_hash,
        hash_csrf_cookie(&issued.csrf_cookie_value)
    );
}

#[test]
fn rejects_a_tampered_state() {
    let key = b"test-signing-key";
    let issued = issue(key);
    let mut tampered = issued.state.clone();
    tampered.push('x');
    assert!(verify(key, &tampered).is_none());
}

#[test]
fn rejects_a_state_signed_with_a_different_key() {
    let issued = issue(b"key-one");
    assert!(verify(b"key-two", &issued.state).is_none());
}

#[test]
fn rejects_malformed_state() {
    assert!(verify(b"key", "not-a-valid-state").is_none());
}

#[test]
fn csrf_hash_does_not_match_a_different_cookie_value() {
    let key = b"test-signing-key";
    let issued = issue(key);
    let verified = verify(key, &issued.state).expect("valid state should verify");
    assert_ne!(
        verified.csrf_hash,
        hash_csrf_cookie("attacker-supplied-value")
    );
}

/// Signs a `state` payload as [`issue`] would, but with a caller-chosen issue time. The signature
/// is genuine, so anything rejected here is rejected on age alone, not because the HMAC failed.
fn state_issued_at(key: &[u8], issued_at: i64) -> String {
    let payload = format!("{issued_at}.{}.{}", "a".repeat(64), "verifier");
    let signature = crate::services::hmac_sign::sign(key, payload.as_bytes());
    format!("{payload}.{signature}")
}

/// The TTL is what bounds how long a captured `state` stays replayable. Without this, a
/// correctly-signed `state` would be accepted forever and the constant would be decoration.
#[test]
fn a_state_older_than_the_ttl_is_rejected() {
    let key = b"test-signing-key";
    let now = chrono::Utc::now().timestamp();

    assert!(
        verify(key, &state_issued_at(key, now - STATE_TTL_SECS)).is_some(),
        "a state exactly at the TTL must still verify"
    );
    assert!(
        verify(key, &state_issued_at(key, now - STATE_TTL_SECS - 1)).is_none(),
        "a state one second past the TTL must be rejected"
    );
}

/// A future-dated `state` cannot have been issued by this process, so accepting one would mean a
/// signing key leak had produced a token that outlives every TTL check. A few seconds of skew are
/// allowed because the issuing and verifying instances are different machines.
#[test]
fn a_future_dated_state_is_rejected_beyond_the_skew_allowance() {
    let key = b"test-signing-key";
    let now = chrono::Utc::now().timestamp();

    assert!(
        verify(key, &state_issued_at(key, now + 5)).is_some(),
        "a state within the skew allowance must verify"
    );
    assert!(
        verify(key, &state_issued_at(key, now + 60)).is_none(),
        "a state dated a minute into the future must be rejected"
    );
}

/// `splitn(4, '.')` puts everything after the third dot into the signature field, so a payload
/// carrying an extra dot would silently shift the parts. The explicit `parts.next().is_some()`
/// guard rejects it instead; this pins that the guard stays.
#[test]
fn a_state_with_an_extra_segment_is_rejected() {
    let key = b"test-signing-key";
    let issued = issue(key);
    assert!(verify(key, &format!("{}.extra", issued.state)).is_none());
}

/// A non-numeric timestamp verifies as a signature but cannot be aged. It has to be rejected
/// rather than treated as time zero, which would make it permanently valid.
#[test]
fn a_state_with_an_unparseable_timestamp_is_rejected() {
    let key = b"test-signing-key";
    let payload = format!("not-a-timestamp.{}.verifier", "a".repeat(64));
    let signature = crate::services::hmac_sign::sign(key, payload.as_bytes());
    assert!(verify(key, &format!("{payload}.{signature}")).is_none());
}
