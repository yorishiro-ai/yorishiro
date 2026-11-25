use yorishiro::models::api_keys::AuthContext;
use yorishiro::models::iam::access;

#[test]
fn the_audit_grant_is_held_only_by_a_key_that_carries_it() {
    for audit in [false, true] {
        let context = AuthContext {
            api_key_id: uuid::Uuid::now_v7(),
            workspace_id: None,
            tenant_id: uuid::Uuid::now_v7(),
            user_id: None,
            audit,
        };
        let result = access::require_audit(&context);
        assert_eq!(result.is_ok(), audit);
        if let Err(error) = result {
            assert!(matches!(
                error,
                yorishiro::YorishiroError::ScopeInsufficient { .. }
            ));
        }
    }
}
