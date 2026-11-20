use axum::http::{HeaderMap, HeaderValue};
use stripe_webhook::Webhook;
use yorishiro::edition::ee::controllers::stripe::inbound::{Error, verify_at};

const SECRET: &str = "whsec_test";
const NOW: i64 = 1_700_000_000;
const EVENT: &str = r#"{"id":"evt_1","type":"test.event","created":1700000000,"livemode":false,"pending_webhooks":1,"data":{"object":{}}}"#;

fn signed_headers(payload: &str, timestamp: i64) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "stripe-signature",
        HeaderValue::from_str(&Webhook::generate_test_header(
            payload,
            SECRET,
            Some(timestamp),
        ))
        .unwrap(),
    );
    headers
}

#[test]
fn verifies_signed_events_and_rejects_tampering() {
    let headers = signed_headers(EVENT, NOW);
    assert!(verify_at(&headers, EVENT.as_bytes(), SECRET, NOW).is_ok());
    assert!(matches!(
        verify_at(&headers, format!("{EVENT} ").as_bytes(), SECRET, NOW),
        Err(Error::InvalidSignature),
    ));
}

#[test]
fn rejects_expired_and_extreme_timestamps() {
    for timestamp in [NOW - 301, NOW + 301, i64::MIN, i64::MAX] {
        assert!(matches!(
            verify_at(
                &signed_headers(EVENT, timestamp),
                EVENT.as_bytes(),
                SECRET,
                NOW
            ),
            Err(Error::InvalidSignature),
        ));
    }
}

#[test]
fn rejects_missing_signature_and_invalid_event() {
    assert!(matches!(
        verify_at(&HeaderMap::new(), EVENT.as_bytes(), SECRET, NOW),
        Err(Error::MissingSignatureHeader),
    ));
    let invalid = "not json";
    assert!(matches!(
        verify_at(
            &signed_headers(invalid, NOW),
            invalid.as_bytes(),
            SECRET,
            NOW
        ),
        Err(Error::InvalidPayload),
    ));
}
