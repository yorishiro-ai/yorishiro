use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;
use yorishiro::db::DbHandle;
use yorishiro::edition::ee::controllers::middleware::auth::WORKSPACE_HEADER;
use yorishiro::edition::ee::models::api_keys::create_tenant_api_key;

/// Installing `TenantScopedAuthenticator` (`App::after_context`) must not break a workspace-scoped key on a route base itself defines.
#[tokio::test]
async fn a_workspace_scoped_key_still_works_on_a_base_route() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key, .. } =
            TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {api_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            200,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// A tenant-scoped key (`workspace_id` NULL) names its workspace per request with `X-Workspace-Id`, and is rejected without one.
#[tokio::test]
async fn a_tenant_scoped_key_resolves_the_workspace_named_by_the_header() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id: user_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        // create_tenant_api_key reads tenant_tenants and tenant_memberships, neither granted to yorishiro_app (see tenant_tenants's migration doc comment), so it runs on the identity pool (the migration/admin role), not the RLS-scoped tenant pool.
        let db = ctx.shared_store.get::<DbHandle>().unwrap();
        let identity = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(db.identity.clone());
        let tenant_key = create_tenant_api_key(&identity, tenant_id, "read", Some(user_id))
            .await
            .expect("issuing a tenant-scoped key");

        // Without the header, `authenticate_api_key(hash, NULL)` finds no workspace to resolve against, so this reaches the caller as an ordinary authentication failure, not a validation error naming the header.
        let no_header = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .await;
        assert_eq!(
            no_header.status_code(),
            401,
            "response: {:?}",
            no_header.text()
        );

        let with_header = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .await;
        assert_eq!(
            with_header.status_code(),
            200,
            "response: {:?}",
            with_header.text()
        );
        use sea_orm::EntityTrait;
        let persisted = yorishiro::models::api_keys::Entity::find_by_id(tenant_key.id)
            .one(&identity)
            .await
            .unwrap()
            .unwrap();
        assert!(
            persisted.last_used_at.is_some(),
            "a read-only request must persist last_used_at outside its rolled-back transaction"
        );
        let missing_target = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .await;
        assert_eq!(missing_target.status_code(), 401);
        let response = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .await;
        assert_eq!(response.status_code(), 200, "{}", response.text());
        let identity = response.json::<serde_json::Value>();
        assert_eq!(identity["workspace_id"], workspace_id.to_string());
        assert_eq!(identity["tenant_id"], tenant_id.to_string());
        assert_eq!(identity["user_id"], user_id.to_string());
        assert_eq!(identity["identity_kind"], "user");
        assert!(identity["bound_workspace_id"].is_null());
        assert_eq!(identity["stored_scope"], "read");
        assert_eq!(identity["scope"], "read");
        assert!(!identity["workspace_name"].as_str().unwrap().is_empty());
        assert!(!identity["tenant_name"].as_str().unwrap().is_empty());
        assert!(!response.text().contains(&tenant_key.plaintext));
        assert!(!response.text().contains("key_hash"));
    })
    .await;
}
