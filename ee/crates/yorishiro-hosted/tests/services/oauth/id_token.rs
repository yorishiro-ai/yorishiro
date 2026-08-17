use super::*;

/// The claims come from an identity provider and drive user provisioning, so the shape is a
/// trust boundary. `sub` is the only field that must be present: it is the stable identity.
#[test]
fn only_the_subject_is_required() {
    let claims: IdTokenClaims =
        serde_json::from_value(serde_json::json!({ "sub": "user-123" })).unwrap();

    assert_eq!(claims.sub, "user-123");
    assert!(claims.email.is_none());
    assert!(!claims.email_verified);
    assert!(claims.name.is_none());
}

/// An id token without a subject cannot identify anyone; it must fail to parse rather than
/// provision an anonymous account.
#[test]
fn a_token_without_a_subject_is_rejected() {
    assert!(
        serde_json::from_value::<IdTokenClaims>(serde_json::json!({ "email": "a@b.c" })).is_err()
    );
}

/// `email_verified` defaults to false when the provider omits it. Defaulting to true would let an
/// unverified address be treated as proven.
#[test]
fn an_absent_email_verified_claim_defaults_to_unverified() {
    let claims: IdTokenClaims =
        serde_json::from_value(serde_json::json!({ "sub": "s", "email": "a@b.c" })).unwrap();

    assert!(!claims.email_verified);
}
