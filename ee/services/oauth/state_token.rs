//! The `state` parameter carried through the OAuth2 authorization-code round trip.
//!
//! No server-side session store backs `/auth/oauth/authorize` and `/auth/oauth/callback`, so the PKCE code verifier is packed into the `state` value itself and HMAC-SHA256 signed via [`crate::ee::services::hmac_sign`], rather than stored server-side and looked up by an opaque id.
//!
//! The signature alone only proves this process issued *some* `state`, not that the browser presenting it is the one the flow was started for.
//! That is what the CSRF cookie is for: `authorize` sets a random, per-browser value as an `HttpOnly`/`Secure`/`SameSite=Lax` cookie and embeds `SHA256(cookie value)` in the signed `state` payload; `callback` recomputes that hash from whatever cookie the browser actually presents and rejects the request if it does not match [`verify`]'s `csrf_hash` output.
//! An attacker who captures a victim's `code`/`state` pair cannot forge the victim's browser's cookie, so the double-submit check fails even though the `state` signature itself is valid.

use crate::services::auth::hex_encode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::ee::services::hmac_sign;

/// How long a `state` value remains acceptable after being issued.
/// Also used as the CSRF cookie's max-age, so the cookie never outlives the `state` that depends on it.
pub const STATE_TTL_SECS: i64 = 600;

/// Number of random bytes in the CSRF cookie value.
/// Only its SHA-256 hash is ever embedded in `state`, so this need not resist offline brute-forcing on its own.
const CSRF_COOKIE_BYTES: usize = 16;

pub struct IssuedState {
    /// The opaque value to embed in the authorize URL's `state` query parameter.
    /// Carries the PKCE code verifier too, so the callback can recover it via [`verify`] without this process having kept anything in memory in between.
    pub state: String,
    /// The PKCE code challenge (`BASE64URL(SHA256(verifier))`) to send in the authorize request.
    pub pkce_challenge: String,
    /// The random value to set as the CSRF cookie.
    /// `state` carries only its SHA-256 hash, never the value itself.
    pub csrf_cookie_value: String,
}

/// The verified result of a `state`: the PKCE verifier it carries, and the CSRF hash it expects the callback's cookie to match.
pub struct VerifiedState {
    pub pkce_verifier: String,
    pub csrf_hash: String,
}

/// Generates a fresh CSRF cookie value and PKCE verifier, and packs the PKCE verifier plus the CSRF value's hash into a signed `state` value.
pub fn issue(signing_key: &[u8]) -> IssuedState {
    let mut csrf_bytes = [0u8; CSRF_COOKIE_BYTES];
    rand::rng().fill_bytes(&mut csrf_bytes);
    let csrf_cookie_value = URL_SAFE_NO_PAD.encode(csrf_bytes);
    let csrf_hash = hex_encode(&Sha256::digest(csrf_cookie_value.as_bytes()));

    let mut verifier_bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut verifier_bytes);
    let pkce_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let pkce_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce_verifier.as_bytes()));

    let issued_at = chrono::Utc::now().timestamp();
    let payload = format!("{issued_at}.{csrf_hash}.{pkce_verifier}");
    let signature = hmac_sign::sign(signing_key, payload.as_bytes());
    let state = format!("{payload}.{signature}");

    IssuedState {
        state,
        pkce_challenge,
        csrf_cookie_value,
    }
}

/// Verifies a `state` value's signature and freshness, returning the PKCE verifier and expected CSRF hash it carries.
/// Rejects anything that does not parse, does not verify, or has aged past [`STATE_TTL_SECS`]: each is indistinguishable from the others to the caller, so a forged, expired or malformed `state` cannot be told apart by an attacker probing the endpoint.
///
/// This alone does **not** prove the presenting browser is the one the flow was started for.
/// See the module docs.
/// Callers must separately check the returned `csrf_hash` against the SHA-256 hash of the browser's CSRF cookie.
pub fn verify(signing_key: &[u8], state: &str) -> Option<VerifiedState> {
    let mut parts = state.splitn(4, '.');
    let issued_at_str = parts.next()?;
    let csrf_hash = parts.next()?;
    let pkce_verifier = parts.next()?;
    let signature = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let payload = format!("{issued_at_str}.{csrf_hash}.{pkce_verifier}");
    if !hmac_sign::verify(signing_key, payload.as_bytes(), signature) {
        return None;
    }

    let issued_at: i64 = issued_at_str.parse().ok()?;
    let now = chrono::Utc::now().timestamp();
    if now - issued_at > STATE_TTL_SECS || issued_at - now > 5 {
        // Small clock-skew allowance in the future direction; not an outright future-dated token.
        return None;
    }

    Some(VerifiedState {
        pkce_verifier: pkce_verifier.to_string(),
        csrf_hash: csrf_hash.to_string(),
    })
}

/// Hashes a CSRF cookie value the same way [`issue`] does, for `callback` to compare against [`VerifiedState::csrf_hash`].
pub fn hash_csrf_cookie(cookie_value: &str) -> String {
    hex_encode(&Sha256::digest(cookie_value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"a signing key";

    #[test]
    fn issued_state_verifies_and_the_csrf_hash_matches_the_cookie() {
        let issued = issue(KEY);
        let verified = verify(KEY, &issued.state).expect("a freshly issued state must verify");
        assert_eq!(
            verified.csrf_hash,
            hash_csrf_cookie(&issued.csrf_cookie_value)
        );
    }

    #[test]
    fn verify_rejects_a_wrong_signing_key() {
        let issued = issue(KEY);
        assert!(verify(b"a different key", &issued.state).is_none());
    }

    #[test]
    fn verify_rejects_a_tampered_payload() {
        let issued = issue(KEY);
        // Flip the last character of the payload without re-signing.
        let mut parts: Vec<&str> = issued.state.split('.').collect();
        let last_payload_part = parts[2].to_string();
        let mut tampered_part = last_payload_part.clone();
        tampered_part.push('x');
        parts[2] = &tampered_part;
        let tampered = parts.join(".");

        assert!(verify(KEY, &tampered).is_none());
    }

    #[test]
    fn verify_rejects_a_malformed_state() {
        assert!(verify(KEY, "not.enough.parts").is_none());
        assert!(verify(KEY, "").is_none());
    }

    #[test]
    fn verify_rejects_an_expired_state() {
        // Hand-built rather than sleeping in the test.
        let issued_at = chrono::Utc::now().timestamp() - STATE_TTL_SECS - 1;
        let csrf_hash = "deadbeef";
        let pkce_verifier = "verifier";
        let payload = format!("{issued_at}.{csrf_hash}.{pkce_verifier}");
        let signature = hmac_sign::sign(KEY, payload.as_bytes());
        let state = format!("{payload}.{signature}");

        assert!(verify(KEY, &state).is_none());
    }

    #[test]
    fn verify_rejects_a_state_issued_too_far_in_the_future() {
        let issued_at = chrono::Utc::now().timestamp() + 10;
        let csrf_hash = "deadbeef";
        let pkce_verifier = "verifier";
        let payload = format!("{issued_at}.{csrf_hash}.{pkce_verifier}");
        let signature = hmac_sign::sign(KEY, payload.as_bytes());
        let state = format!("{payload}.{signature}");

        assert!(verify(KEY, &state).is_none());
    }
}
