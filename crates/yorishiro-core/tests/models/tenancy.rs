use super::*;

/// `as_db_str` and `from_db_str` are `pub(crate)`, so this pairing could not be asserted from an
/// external integration test at all. They straddle the `identity.tenant_memberships.role` check
/// constraint: a value that does not round-trip here is a row the database will reject.
#[test]
fn every_role_round_trips_through_its_database_representation() {
    for role in [
        MembershipRole::Owner,
        MembershipRole::Admin,
        MembershipRole::Member,
        MembershipRole::Viewer,
    ] {
        let stored = role.as_db_str();
        assert_eq!(MembershipRole::from_db_str(stored), Some(role));
    }
}

/// The stored strings are the constraint's literal values, not an implementation detail -- they
/// are pinned so a rename cannot silently start writing rows the constraint rejects.
#[test]
fn the_database_representation_matches_the_check_constraint() {
    assert_eq!(MembershipRole::Owner.as_db_str(), "owner");
    assert_eq!(MembershipRole::Admin.as_db_str(), "admin");
    assert_eq!(MembershipRole::Member.as_db_str(), "member");
    assert_eq!(MembershipRole::Viewer.as_db_str(), "viewer");
}

/// A role read back from a row written by a newer version must not silently become a valid role.
#[test]
fn an_unknown_role_string_is_rejected_rather_than_defaulted() {
    assert_eq!(MembershipRole::from_db_str("superuser"), None);
    assert_eq!(MembershipRole::from_db_str(""), None);
    assert_eq!(MembershipRole::from_db_str("Owner"), None);
}

/// The JSON form is lowercase via `#[serde(rename_all)]`, and it is what API clients send and
/// receive -- it must stay aligned with the database form rather than drifting from it.
#[test]
fn the_json_representation_is_lowercase_and_matches_the_database_form() {
    for role in [
        MembershipRole::Owner,
        MembershipRole::Admin,
        MembershipRole::Member,
        MembershipRole::Viewer,
    ] {
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, format!("\"{}\"", role.as_db_str()));

        let parsed: MembershipRole = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, role);
    }
}
