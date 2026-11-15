use yorishiro::models::access;
use yorishiro::models::api_keys::{ApiKeyScope, AuthContext};

#[test]
fn audit_and_content_scope_are_independent() {
    let mut context = AuthContext {
        api_key_id: uuid::Uuid::nil(),
        workspace_id: Some(uuid::Uuid::nil()),
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
