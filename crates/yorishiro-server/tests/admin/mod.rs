use super::*;

/// The scope/role arguments are the CLI's surface for values the database constrains.
/// Their conversions are what stop `admin create-api-key <ws> write` from writing a value the column rejects, so each arm is pinned.
#[test]
fn scope_arguments_convert_to_their_core_scope() {
    use yorishiro_core::services::auth::ApiKeyScope;

    assert_eq!(ApiKeyScope::from(ScopeArg::Read), ApiKeyScope::Read);
    assert_eq!(ApiKeyScope::from(ScopeArg::Write), ApiKeyScope::Write);
    assert_eq!(ApiKeyScope::from(ScopeArg::Schema), ApiKeyScope::Schema);
}

/// Same for roles: the CLI accepts four, and they must map onto the four the membership check constraint permits.
#[test]
fn role_arguments_convert_to_their_core_role() {
    use yorishiro_core::repositories::tenancy::MembershipRole;

    assert_eq!(MembershipRole::from(RoleArg::Owner), MembershipRole::Owner);
    assert_eq!(MembershipRole::from(RoleArg::Admin), MembershipRole::Admin);
    assert_eq!(
        MembershipRole::from(RoleArg::Member),
        MembershipRole::Member
    );
    assert_eq!(
        MembershipRole::from(RoleArg::Viewer),
        MembershipRole::Viewer
    );
}
