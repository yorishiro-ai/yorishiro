use axum::http::HeaderMap;
use yorishiro::edition::ee::controllers::hmac_sign;
use yorishiro::edition::ee::controllers::stripe::SIGNATURE_TOLERANCE_SECS;

use axum::http::HeaderValue;
use yorishiro::edition::ee::controllers::stripe::inbound::*;

const SECRET: &str = "whsec_test";
const NOW: i64 = 1_700_000_000;

fn headers(timestamp: i64, signatures: &[&str]) -> HeaderMap {
    let value = std::iter::once(format!("t={timestamp}"))
        .chain(signatures.iter().map(|signature| format!("v1={signature}")))
        .collect::<Vec<_>>()
        .join(",");
    header(&value)
}

fn header(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("stripe-signature", HeaderValue::from_str(value).unwrap());
    headers
}

fn signed_header(timestamp: i64, payload: &[u8]) -> String {
    let mut signed_payload = format!("{timestamp}.").into_bytes();
    signed_payload.extend_from_slice(payload);
    hmac_sign::sign(SECRET.as_bytes(), &signed_payload)
}

fn event() -> Vec<u8> {
    br#"{"id":"evt_1","type":"test.event","created":1700000000,"data":{"object":{}}}"#.to_vec()
}

#[test]
fn rejects_missing_and_malformed_headers() {
    let payload = event();
    let missing = verify_at(&HeaderMap::new(), &payload, SECRET, NOW).unwrap_err();
    assert!(matches!(missing, Error::MissingSignatureHeader));

    let mut malformed = HeaderMap::new();
    malformed.insert("stripe-signature", HeaderValue::from_static("v1=abc"));
    let error = verify_at(&malformed, &payload, SECRET, NOW).unwrap_err();
    assert!(matches!(
        error,
        Error::Signature("missing timestamp in Stripe-Signature header")
    ));
}

#[test]
fn rejects_duplicate_physical_headers_duplicate_t_and_malformed_segments() {
    let payload = event();
    let signature = signed_header(NOW, &payload);

    let mut duplicate_headers = headers(NOW, &[&signature]);
    duplicate_headers.append(
        "stripe-signature",
        HeaderValue::from_str(&format!("t={NOW},v1={signature}")).unwrap(),
    );
    assert!(matches!(
        verify_at(&duplicate_headers, &payload, SECRET, NOW),
        Err(Error::Signature("invalid Stripe-Signature header"))
    ));

    assert!(matches!(
        verify_at(
            &header(&format!("t={NOW},t={NOW},v1={signature}")),
            &payload,
            SECRET,
            NOW
        ),
        Err(Error::Signature("invalid Stripe-Signature header"))
    ));

    for suffix in [",", ",,foo=bar", ",foo", ",foo=bar=baz", ",=bar", ",foo="] {
        assert!(matches!(
            verify_at(
                &header(&format!("t={NOW},v1={signature}{suffix}")),
                &payload,
                SECRET,
                NOW
            ),
            Err(Error::Signature("invalid Stripe-Signature header"))
        ));
    }
}

#[test]
fn rejects_invalid_utf8_and_extreme_timestamps_without_overflow() {
    let payload = event();
    let mut invalid_utf8 = HeaderMap::new();
    invalid_utf8.insert(
        "stripe-signature",
        HeaderValue::from_bytes(&[0xff]).unwrap(),
    );
    assert!(matches!(
        verify_at(&invalid_utf8, &payload, SECRET, NOW),
        Err(Error::MissingSignatureHeader)
    ));

    for timestamp in [i64::MIN, i64::MAX] {
        let signature = signed_header(timestamp, &payload);
        assert!(matches!(
            verify_at(&headers(timestamp, &[&signature]), &payload, SECRET, NOW),
            Err(Error::Signature(
                "Stripe-Signature timestamp is outside the allowed tolerance"
            ))
        ));
    }
}

#[test]
fn rejects_invalid_signatures_and_accepts_one_of_multiple_signatures() {
    let payload = event();
    let valid = signed_header(NOW, &payload);
    let invalid = headers(NOW, &["not-hex"]);
    assert!(matches!(
        verify_at(&invalid, &payload, SECRET, NOW),
        Err(Error::Signature(
            "no v1 signature matched the computed HMAC"
        ))
    ));

    let multiple = headers(NOW, &["not-hex", &valid]);
    assert!(verify_at(&multiple, &payload, SECRET, NOW).is_ok());

    let non_v1 = header(&format!("t={NOW},v0=legacy,v1={valid}"));
    assert!(verify_at(&non_v1, &payload, SECRET, NOW).is_ok());
}

#[test]
fn accepts_tolerance_boundaries_and_rejects_outside_them() {
    let payload = event();
    for timestamp in [
        NOW - SIGNATURE_TOLERANCE_SECS,
        NOW + SIGNATURE_TOLERANCE_SECS,
    ] {
        let signature = signed_header(timestamp, &payload);
        assert!(verify_at(&headers(timestamp, &[&signature]), &payload, SECRET, NOW).is_ok());
    }

    for timestamp in [
        NOW - SIGNATURE_TOLERANCE_SECS - 1,
        NOW + SIGNATURE_TOLERANCE_SECS + 1,
    ] {
        let signature = signed_header(timestamp, &payload);
        assert!(matches!(
            verify_at(&headers(timestamp, &[&signature]), &payload, SECRET, NOW),
            Err(Error::Signature(
                "Stripe-Signature timestamp is outside the allowed tolerance"
            ))
        ));
    }
}

#[test]
fn parses_only_after_signature_verification() {
    let payload = br#"not json"#;
    let signature = signed_header(NOW, payload);
    let error = verify_at(&headers(NOW, &[&signature]), payload, SECRET, NOW).unwrap_err();
    assert!(matches!(error, Error::InvalidJson(_)));
}
