//! Shared test fixtures for tenant/workspace/owner creation.
//!
//! A ~20-line block that creates a tenant, workspace, and owner is repeated
//! across multiple test files. This module provides a builder that encapsulates
//! the shared setup logic while letting each caller specify its own key scope
//! and return shape.

use loco_rs::app::AppContext;
use sea_orm::TransactionTrait;
use uuid::Uuid;
use yorishiro::models::_entities::api_keys;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenancy;

pub struct TestTenant {
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub owner_id: Uuid,
    pub api_key: String,
}

impl TestTenant {
    pub fn authenticated(&self, mut request: loco_rs::TestServer) -> loco_rs::TestServer {
        request.add_header("Authorization", format!("Bearer {}", self.api_key));
        request
    }

    pub async fn create(request: &loco_rs::TestServer, ctx: &AppContext, args: TenantArgs) -> Self {
        #[cfg(not(feature = "enterprise"))]
        if ctx.config.database.uri.starts_with("postgres:")
            && std::env::var("DATABASE_URL").is_ok_and(|url| url == ctx.config.database.uri)
        {
            use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
            use yorishiro::models::{tenant_memberships, tenant_tenants, user_users};

            static TENANT: tokio::sync::OnceCell<Uuid> = tokio::sync::OnceCell::const_new();
            let tenant_id = *TENANT
                .get_or_init(|| async {
                    if let Some(tenant) = tenant_tenants::Entity::find()
                        .filter(
                            yorishiro::models::_entities::tenant_tenants::Column::Id
                                .ne(Uuid::nil()),
                        )
                        .one(&ctx.db)
                        .await
                        .expect("read shared CE tenant")
                    {
                        return tenant.id;
                    }
                    let response = request
                        .post("/auth/signup")
                        .json(&serde_json::json!({
                            "email": unique_email("shared-ce"),
                            "password": "hunter2-hunter2",
                            "display_name": "shared CE tenant",
                        }))
                        .await;
                    response.assert_status(axum::http::StatusCode::CREATED);
                    serde_json::from_value(
                        response.json::<serde_json::Value>()["tenant_id"].clone(),
                    )
                    .expect("shared CE tenant ID")
                })
                .await;
            let txn = ctx.db.begin().await.expect("begin CE workspace setup");
            let owner = user_users::create_user(
                &txn,
                &args.owner_email,
                &args.owner_password,
                Some(&args.tenant_name),
            )
            .await
            .expect("create CE fixture owner");
            tenant_memberships::add_member(
                &txn,
                tenant_id,
                owner.id,
                tenant_memberships::MembershipRole::Owner,
            )
            .await
            .expect("add CE fixture owner");
            let workspace =
                tenancy::create_workspace(&txn, tenant_id, &args.workspace_name, None, None)
                    .await
                    .expect("create CE fixture workspace");
            txn.commit().await.expect("commit CE workspace setup");
            return Self {
                tenant_id,
                workspace_id: workspace.id,
                owner_id: owner.id,
                api_key: issue_api_key(ctx, workspace.id, owner.id, args.key_scope, args.key_audit)
                    .await,
            };
        }
        let response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "email": args.owner_email,
                "password": args.owner_password,
                "display_name": args.tenant_name,
            }))
            .await;
        response.assert_status(axum::http::StatusCode::CREATED);
        let account = response.json::<serde_json::Value>();
        let tenant_id =
            serde_json::from_value(account["tenant_id"].clone()).expect("signup tenant ID");
        let owner_id = serde_json::from_value(account["user_id"].clone()).expect("signup owner ID");
        let txn = ctx.db.begin().await.expect("begin workspace setup");
        let workspace =
            tenancy::create_workspace(&txn, tenant_id, &args.workspace_name, None, None)
                .await
                .expect("create workspace");
        txn.commit().await.expect("commit workspace setup");
        let api_key =
            issue_api_key(ctx, workspace.id, owner_id, args.key_scope, args.key_audit).await;
        Self {
            tenant_id,
            workspace_id: workspace.id,
            owner_id,
            api_key,
        }
    }
}

/// Arguments for shared tenant/workspace/owner setup.
pub struct TenantArgs {
    /// Tenant name. Defaults to a unique name.
    pub tenant_name: String,
    /// Workspace name. Defaults to a unique name.
    pub workspace_name: String,
    /// Owner email. Defaults to a unique address: users are unique across the shared test database.
    pub owner_email: String,
    /// Owner password. Defaults to `"hunter2-hunter2"`.
    pub owner_password: String,
    /// API key scope. Defaults to `Migration`.
    pub key_scope: ApiKeyScope,
    /// Whether the audit key is audit-scoped.
    pub key_audit: bool,
}

impl Default for TenantArgs {
    fn default() -> Self {
        Self {
            tenant_name: format!("tenant-{}", Uuid::new_v4().simple()),
            workspace_name: format!("workspace-{}", Uuid::new_v4().simple()),
            owner_email: unique_email("owner"),
            owner_password: "hunter2-hunter2".into(),
            key_scope: ApiKeyScope::Migration,
            key_audit: false,
        }
    }
}

/// Issue an API key for an existing user.
///
/// Create a `TestTenant` first, then use its owner ID for additional scoped keys.
pub async fn issue_api_key(
    ctx: &AppContext,
    workspace_id: Uuid,
    user_id: Uuid,
    scope: ApiKeyScope,
    audit: bool,
) -> String {
    api_keys::Entity::create_api_key(&ctx.db, workspace_id, scope, Some(user_id), audit)
        .await
        .expect("issue api key")
        .plaintext
}

/// An email address no other test in the shared database uses.
pub fn unique_email(prefix: &str) -> String {
    format!("{prefix}-{}@example.com", Uuid::new_v4().simple())
}
