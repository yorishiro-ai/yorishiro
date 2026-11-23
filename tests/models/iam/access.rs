use yorishiro::models::api_keys::{ApiKeyScope, AuthContext};
use yorishiro::models::iam::access;

#[test]
fn every_content_scope_enforces_its_boundary_independently_of_audit() {
    let scopes = [
        ApiKeyScope::Read,
        ApiKeyScope::Write,
        ApiKeyScope::Schema,
        ApiKeyScope::Migration,
    ];
    for (held, scope) in scopes.iter().enumerate() {
        for audit in [false, true] {
            let context = AuthContext {
                api_key_id: uuid::Uuid::now_v7(),
                workspace_id: None,
                tenant_id: uuid::Uuid::now_v7(),
                scope: *scope,
                user_id: None,
                audit,
            };
            for (required, required_scope) in scopes.iter().enumerate() {
                match access::require_scope(&context, *required_scope) {
                    Ok(()) => assert!(held >= required),
                    Err(error) => {
                        assert!(held < required);
                        assert!(matches!(
                            error,
                            yorishiro::YorishiroError::ScopeInsufficient { .. }
                        ));
                    }
                }
            }
            assert_eq!(access::require_audit(&context).is_ok(), audit);
        }
    }
}
