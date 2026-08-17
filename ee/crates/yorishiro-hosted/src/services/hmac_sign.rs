//! Shared HMAC-SHA256 sign/verify used by both the OAuth `state` token (`services::oauth::state_token`) and the Stripe webhook signature (`http::controllers::stripe`): the same primitive and verification style for both.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use yorishiro_core::services::auth::hex_encode;

type HmacSha256 = Hmac<Sha256>;

/// Computes the lowercase-hex HMAC-SHA256 of `payload` under `key`.
pub fn sign(key: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(payload);
    hex_encode(&mac.finalize().into_bytes())
}

/// Verifies that `candidate_hex` is the HMAC-SHA256 of `payload` under `key`, using the `hmac` crate's constant-time `verify_slice` rather than comparing hex strings byte-by-byte.
pub fn verify(key: &[u8], payload: &[u8], candidate_hex: &str) -> bool {
    let Some(candidate_bytes) = yorishiro_core::services::auth::hex_decode(candidate_hex) else {
        return false;
    };
    let Ok(mac) = HmacSha256::new_from_slice(key) else {
        return false;
    };
    mac.chain_update(payload)
        .verify_slice(&candidate_bytes)
        .is_ok()
}

#[cfg(test)]
#[path = "../../tests/services/hmac_sign.rs"]
mod tests;
