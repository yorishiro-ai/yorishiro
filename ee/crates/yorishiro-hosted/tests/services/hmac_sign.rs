use super::*;

#[test]
fn verify_accepts_a_freshly_signed_payload() {
    let key = b"test-key";
    let signature = sign(key, b"hello world");
    assert!(verify(key, b"hello world", &signature));
}

#[test]
fn verify_rejects_a_tampered_payload() {
    let key = b"test-key";
    let signature = sign(key, b"hello world");
    assert!(!verify(key, b"hello mars", &signature));
}

#[test]
fn verify_rejects_a_signature_from_a_different_key() {
    let signature = sign(b"key-one", b"hello world");
    assert!(!verify(b"key-two", b"hello world", &signature));
}

#[test]
fn verify_rejects_non_hex_candidate() {
    let key = b"test-key";
    assert!(!verify(key, b"hello world", "not-hex!!"));
}
