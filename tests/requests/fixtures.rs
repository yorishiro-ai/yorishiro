//! Shared test fixtures for tenant/workspace/owner creation.
//!
//! Uses signup for new tenants.

use loco_rs::app::AppContext;
use sea_orm::TransactionTrait;
use uuid::Uuid;
use yorishiro::models::_entities::api_keys;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenancy;

pub(crate) async fn grant_workspace(
    ctx: &AppContext,
    workspace_id: Uuid,
    user_id: Uuid,
    role: yorishiro::models::workspace_memberships::WorkspaceRole,
) {
    // SQLite fails a deferred transaction that upgrades to a write after another connection has written; take the writer slot up front.
    let txn = ctx
        .db
        .begin_with_options(sea_orm::TransactionOptions {
            sqlite_transaction_mode: Some(sea_orm::SqliteTransactionMode::Immediate),
            ..Default::default()
        })
        .await
        .expect("begin workspace grant");
    yorishiro::models::workspace_memberships::Model::grant(&txn, workspace_id, user_id, role)
        .await
        .expect("grant explicit workspace access");
    txn.commit().await.expect("commit workspace grant");
}

/// Caps the tenant's workspaces.
/// No public operation changes this cap (only a plan does), so this is the one place a test writes it directly.
pub(crate) async fn set_workspace_cap(ctx: &AppContext, tenant_id: Uuid, cap: i32) {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    yorishiro::models::tenant_tenants::ActiveModel {
        id: Set(tenant_id),
        max_workspaces: Set(Some(cap)),
        ..Default::default()
    }
    .update(&ctx.db)
    .await
    .expect("set the workspace cap");
}

/// Disables the tenant through the operator operation the `disable_tenant` task runs.
pub(crate) async fn disable_tenant(ctx: &AppContext, tenant_id: Uuid) {
    yorishiro::models::tenant_tenants::retirement::disable_by_operator(
        &ctx.db, tenant_id, tenant_id,
    )
    .await
    .expect("disable the tenant");
}

pub(crate) struct TestTenant {
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub owner_id: Uuid,
    pub api_key: String,
}

impl TestTenant {
    #[expect(
        clippy::future_not_send,
        reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
    )]
    pub(crate) async fn create(
        request: &loco_rs::TestServer,
        ctx: &AppContext,
        args: TenantArgs,
    ) -> Self {
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

pub(crate) async fn grant_iam_administrator(
    txn: &sea_orm::DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) {
    use sea_orm::{ActiveModelTrait, Set};
    let policy = yorishiro::models::iam_policies::ActiveModel {
        tenant_id: Set(tenant_id),
        name: Set(format!("fixture-admin-{user_id}")),
        document: Set(
            serde_json::json!({"statements":[{"effect":"allow","actions":["*"],
            "resources":[{"type":"tenant","tenant_id":tenant_id},
                {"type":"all_workspaces","tenant_id":tenant_id},
                {"type":"all_roles","tenant_id":tenant_id}]}]}),
        ),
        ..Default::default()
    }
    .insert(txn)
    .await
    .expect("create fixture administrator policy");
    yorishiro::models::iam_user_policy_attachments::ActiveModel {
        tenant_id: Set(tenant_id),
        user_id: Set(user_id),
        policy_id: Set(policy.id),
        ..Default::default()
    }
    .insert(txn)
    .await
    .expect("attach fixture administrator policy");
}

/// Arguments for shared tenant/workspace/owner setup.
pub(crate) struct TenantArgs {
    /// Tenant name. Defaults to a unique name.
    pub tenant_name: String,
    /// Workspace name. Defaults to a unique name.
    pub workspace_name: String,
    /// Owner email. Defaults to a unique address.
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
pub(crate) async fn issue_api_key(
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

pub(crate) async fn issue_tenant_key(ctx: &AppContext, tenant_id: Uuid, user_id: Uuid) -> String {
    api_keys::Entity::create_tenant_api_key(
        &ctx.db,
        tenant_id,
        ApiKeyScope::Migration,
        Some(user_id),
        false,
        "test-tenant-admin",
    )
    .await
    .expect("issue tenant key")
    .plaintext
}

/// An email address no other test in the same database uses.
pub(crate) fn unique_email(prefix: &str) -> String {
    format!("{prefix}-{}@example.com", Uuid::new_v4().simple())
}

/// A one-field `note` schema definition named `name`; `embedded` marks its title for embedding.
pub(crate) fn note_schema(name: &str, embedded: bool) -> serde_json::Value {
    let mut title = serde_json::json!({"type": "string", "required": true});
    if embedded {
        title["x-embed"] = true.into();
    }
    serde_json::json!({
        "name": name,
        "entity_types": {"note": {"fields": {"title": title}}}
    })
}
