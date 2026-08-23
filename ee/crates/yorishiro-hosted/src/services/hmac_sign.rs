//! Shared HMAC-SHA256 verify, used by the Stripe webhook signature (`controllers::stripe`).
//! Ported from master's `ee/crates/yorishiro-hosted/src/services/hmac_sign.rs`, minus `sign`:
//! master's copy also backs the OAuth `state` token's signing side, which isn't ported here yet.
//! Re-add `sign` when that lands, rather than leaving an uncalled function in the meantime.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verifies that `candidate_hex` is the HMAC-SHA256 of `payload` under `key`, using the `hmac`
/// crate's constant-time `verify_slice` rather than comparing hex strings byte-by-byte.
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
