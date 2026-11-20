//! HMAC-SHA256 for OAuth `state` tokens.

use aws_lc_rs::hmac::{self, HMAC_SHA256, Key};

/// Computes the lowercase-hex HMAC-SHA256 of `payload` under `key`.
#[must_use]
pub fn sign(key: &[u8], payload: &[u8]) -> String {
    hex::encode(hmac::sign(&Key::new(HMAC_SHA256, key), payload).as_ref())
}

/// Verifies the hex-encoded HMAC-SHA256 tag using AWS-LC's constant-time comparison.
#[must_use]
pub fn verify(key: &[u8], payload: &[u8], candidate_hex: &str) -> bool {
    let mut candidate_bytes = [0; 32];
    if hex::decode_to_slice(candidate_hex, &mut candidate_bytes).is_err() {
        return false;
    }
    hmac::verify(&Key::new(HMAC_SHA256, key), payload, &candidate_bytes).is_ok()
}
