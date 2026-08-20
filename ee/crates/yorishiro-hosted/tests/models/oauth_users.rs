use super::*;

/// Provisioning has to tell a genuine email collision apart from any other failure: the first is something the user can act on ("that address already has an account"), the second is not.
/// A single opaque error would force the caller to string-match.
#[test]
fn a_unique_violation_is_distinguishable_from_other_failures() {
    fn classify(error: &CreateOauthUserError) -> &'static str {
        match error {
            CreateOauthUserError::UniqueViolation => "email already claimed",
            CreateOauthUserError::Other(_) => "other",
        }
    }

    assert_eq!(
        classify(&CreateOauthUserError::UniqueViolation),
        "email already claimed"
    );
    assert_eq!(
        classify(&CreateOauthUserError::Other(
            yorishiro_core::YorishiroError::Unauthenticated
        )),
        "other"
    );
}

/// The doc comment on `UniqueViolation` reasons that only the email constraint can fire, because an advisory lock rules out a concurrent insert of the same identity.
/// That reasoning is what makes the variant safe to report as an email collision, so the variant carries no constraint name to disambiguate: pinned here so a future change that needs one is a deliberate choice.
#[test]
fn the_unique_violation_variant_carries_no_payload() {
    let error = CreateOauthUserError::UniqueViolation;

    assert!(matches!(error, CreateOauthUserError::UniqueViolation));
}
