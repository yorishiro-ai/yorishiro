use super::*;
use crate::services::auth::{ApiKeyScope, AuthContext};

fn ctx_with(scope: ApiKeyScope) -> AuthContext {
    AuthContext {
        api_key_id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
        tenant_id: uuid::Uuid::nil(),
        scope,
        user_id: None,
    }
}

/// `require_scope` is the gate every REST and MCP handler passes through, and it is pure (no database involved), so the whole ordering matrix is worth pinning exactly.
/// Read scope must not satisfy write, and write must not satisfy schema.
#[test]
fn a_key_satisfies_its_own_scope_and_every_weaker_one() {
    let cases = [
        (ApiKeyScope::Read, ApiKeyScope::Read, true),
        (ApiKeyScope::Read, ApiKeyScope::Write, false),
        (ApiKeyScope::Read, ApiKeyScope::Schema, false),
        (ApiKeyScope::Write, ApiKeyScope::Read, true),
        (ApiKeyScope::Write, ApiKeyScope::Write, true),
        (ApiKeyScope::Write, ApiKeyScope::Schema, false),
        (ApiKeyScope::Schema, ApiKeyScope::Read, true),
        (ApiKeyScope::Schema, ApiKeyScope::Write, true),
        (ApiKeyScope::Schema, ApiKeyScope::Schema, true),
    ];

    for (held, required, expected_ok) in cases {
        let result = require_scope(&ctx_with(held), required);
        assert_eq!(
            result.is_ok(),
            expected_ok,
            "holding {held:?} against required {required:?}"
        );
    }
}

/// A rejection has to be a `ScopeInsufficient` (403), not an authentication failure (401): the caller is known, it simply is not permitted, and the two map to different HTTP statuses.
#[test]
fn an_insufficient_scope_is_rejected_as_forbidden_not_unauthenticated() {
    let error = require_scope(&ctx_with(ApiKeyScope::Read), ApiKeyScope::Schema).unwrap_err();

    assert!(matches!(error, YorishiroError::ScopeInsufficient { .. }));
    assert_eq!(error.into_http_parts().0, 403);
}

/// The rejection is actionable only if it says what was needed and what was held, and offers the remedy: this message is what a user sees when their key is too weak.
#[test]
fn the_rejection_names_both_scopes_and_offers_a_remedy() {
    let error = require_scope(&ctx_with(ApiKeyScope::Read), ApiKeyScope::Write).unwrap_err();

    let (_, body) = error.into_http_parts();
    let message = body["error"]["message"].as_str().unwrap().to_string();
    assert!(message.contains("Write"), "{message}");
    assert!(message.contains("Read"), "{message}");
    assert!(!body["error"]["hint"].is_null());
}
