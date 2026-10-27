use yorishiro::models::access;
use yorishiro::models::api_keys::{ApiKeyScope, AuthContext};
use yorishiro::models::tenant_memberships::MembershipRole;

#[test]
fn effective_scope_never_exceeds_either_grant() {
    for (role, cap) in [
        (MembershipRole::Owner, ApiKeyScope::Migration),
        (MembershipRole::Admin, ApiKeyScope::Migration),
        (MembershipRole::Member, ApiKeyScope::Write),
        (MembershipRole::Viewer, ApiKeyScope::Read),
    ] {
        for stored in [
            ApiKeyScope::Read,
            ApiKeyScope::Write,
            ApiKeyScope::Schema,
            ApiKeyScope::Migration,
        ] {
            assert_eq!(access::effective_scope(stored, role), stored.min(cap));
        }
    }
}

#[test]
fn audit_and_content_scope_are_independent() {
    let mut context = AuthContext {
        api_key_id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        tenant_id: uuid::Uuid::nil(),
        scope: ApiKeyScope::Read,
        user_id: None,
        audit: true,
    };
    assert!(access::require_audit(&context).is_ok());
    assert!(access::require_scope(&context, ApiKeyScope::Write).is_err());
    context.scope = ApiKeyScope::Migration;
    context.audit = false;
    assert!(access::require_audit(&context).is_err());
    assert!(access::require_scope(&context, ApiKeyScope::Migration).is_ok());
}
