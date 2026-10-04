//! Shared HMAC-SHA256 sign/verify, used by both the OAuth `state` token (`controllers::oauth::state_token`) and the Stripe webhook signature (`controllers::stripe`).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Computes the lowercase-hex HMAC-SHA256 of `payload` under `key`.
///
/// # Panics
/// Panics if an internal invariant required by this operation is violated.
pub fn sign(key: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(payload);
    hex::encode(mac.finalize().into_bytes())
}

/// Verifies that `candidate_hex` is the HMAC-SHA256 of `payload` under `key`, using the `hmac` crate's constant-time `verify_slice` rather than comparing hex strings byte-by-byte.
pub fn verify(key: &[u8], payload: &[u8], candidate_hex: &str) -> bool {
    let Ok(candidate_bytes) = hex::decode(candidate_hex) else {
        return false;
    };
    let Ok(mac) = HmacSha256::new_from_slice(key) else {
        return false;
    };
    mac.chain_update(payload)
        .verify_slice(&candidate_bytes)
        .is_ok()
}
