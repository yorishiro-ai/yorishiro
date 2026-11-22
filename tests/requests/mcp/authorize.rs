use serde_json::json;
use yorishiro::App;

use crate::requests::boot_request;

use super::*;

#[tokio::test]
async fn mcp_authentication_database_failure_is_redacted() {
    use sea_orm::ConnectionTrait;

    if !crate::require_sqlite_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
        let session = initialize(&request).await;
        ctx.db
            .execute_unprepared("ALTER TABLE api_keys RENAME TO private_auth_diagnostic")
            .await
            .expect("break authentication lookup in this isolated test database");
        let response = mcp_call(
            &request,
            &session,
            "not-a-real-key",
            2,
            "tools/call",
            json!({"name": "list_entities", "arguments": {}}),
        )
        .await;
        assert_eq!(response["error"]["message"], "internal server error");
        assert!(!response.to_string().contains("api_keys"));
        assert!(!response.to_string().contains("private_auth_diagnostic"));
    })
    .await;
}

#[tokio::test]
async fn mcp_authenticator_permission_denials_are_structured_not_internal_errors() {
    use std::sync::Arc;
    use yorishiro::controllers::middleware::auth::Authenticator;

    struct PermissionDenied;
    #[loco_rs::prelude::async_trait]
    impl Authenticator for PermissionDenied {
        async fn authenticate(
            &self,
            _app: &loco_rs::app::AppContext,
            _key: &str,
            _headers: &[(String, String)],
        ) -> Result<yorishiro::models::api_keys::AuthContext, yorishiro::YorishiroError> {
            Err(yorishiro::YorishiroError::ScopeInsufficient {
                message: "workspace access denied".into(),
                hint: "request a workspace grant".into(),
            })
        }
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let session = initialize(&request).await;
        ctx.shared_store
            .insert(Arc::new(PermissionDenied) as Arc<dyn Authenticator>);
        for (id, name) in [(2, "list_entities"), (3, "whoami")] {
            let result = mcp_call(
                &request,
                &session,
                "denied-key",
                id,
                "tools/call",
                json!({"name":name,"arguments":{}}),
            )
            .await;
            assert_eq!(result["result"]["isError"], true, "{result}");
            let error = &result["result"]["structuredContent"]["error"];
            assert_eq!(error["code"], "scope_insufficient");
            assert_eq!(error["message"], "workspace access denied");
            assert_eq!(error["hint"], "request a workspace grant");
            assert!(error.get("required_scope").is_none());
            assert!(error.get("effective_scope").is_none());
        }
    })
    .await;
}
