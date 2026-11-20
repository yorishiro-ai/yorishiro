use yorishiro::edition::ee::data::hmac_sign::*;

#[test]
fn sign_and_verify_round_trip() {
    let key = b"a signing key";
    let payload = b"a payload";
    let signature = sign(key, payload);
    assert!(verify(key, payload, &signature));
    assert!(verify(key, payload, &signature.to_uppercase()));
}

#[test]
fn verify_rejects_a_wrong_key_payload_or_signature() {
    let key = b"a signing key";
    let payload = b"a payload";
    let signature = sign(key, payload);

    assert!(!verify(b"a different key", payload, &signature));
    assert!(!verify(key, b"a different payload", &signature));
    assert!(!verify(key, payload, "not even hex"));
    assert!(!verify(key, payload, &signature[..signature.len() - 1]));
    assert!(!verify(key, payload, &format!("{signature}00")));
    assert!(!verify(key, payload, &"z".repeat(64)));
}
