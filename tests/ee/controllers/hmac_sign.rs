use yorishiro::edition::ee::controllers::hmac_sign::*;

#[test]
fn sign_and_verify_round_trip() {
    let key = b"a signing key";
    let payload = b"a payload";
    let signature = sign(key, payload);
    assert!(verify(key, payload, &signature));
}

#[test]
fn verify_rejects_a_wrong_key_payload_or_signature() {
    let key = b"a signing key";
    let payload = b"a payload";
    let signature = sign(key, payload);

    assert!(!verify(b"a different key", payload, &signature));
    assert!(!verify(key, b"a different payload", &signature));
    assert!(!verify(key, payload, "not even hex"));
    assert!(!verify(key, payload, &sign(b"a different key", payload)));
}
