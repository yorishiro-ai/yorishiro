//! Shared test fixtures for tenant/workspace/owner creation.
//!
//! A ~20-line block that creates a tenant, workspace, and owner is repeated
//! across multiple test files. This module provides a builder that encapsulates
//! the shared setup logic while letting each caller specify its own key scope
//! and return shape.

// This module is compiled as part of the test binary, so clippy warns about
// unused items when no test file has been updated to call the shared fixture
// yet. The dead_code allowances apply only to this file.
#![allow(dead_code)]

use loco_rs::app::AppContext;
use uuid::Uuid;
use yorishiro::models::_entities::{api_keys, tenant_tenants, workspace_workspaces};
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::auth::ApiKeyScope;

/// Arguments for shared tenant/workspace/owner setup.
pub struct TenantArgs {
    /// Tenant name. Defaults to `"acme"`.
    pub tenant_name: String,
    /// Workspace name. Defaults to `"main"`.
    pub workspace_name: String,
    /// Owner email. Defaults to `"owner@example.com"`.
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
            tenant_name: "acme".into(),
            workspace_name: "main".into(),
            owner_email: "owner@example.com".into(),
            owner_password: "hunter2-hunter2".into(),
            key_scope: ApiKeyScope::Migration,
            key_audit: false,
        }
    }
}

/// Shared setup logic for tenant, workspace, owner, and API key.
///
/// Returns `(tenant_id, workspace_id, owner_id, plaintext_key)`.
pub async fn create_tenant_workspace_owner(
    ctx: &AppContext,
    args: TenantArgs,
) -> (Uuid, Uuid, Uuid, String) {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(args.tenant_name),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");

    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set(args.workspace_name),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");

    let owner = tenancy::create_user(&ctx.db, &args.owner_email, &args.owner_password, None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");

    let plaintext = api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        args.key_scope,
        Some(owner.id),
        args.key_audit,
    )
    .await
    .expect("issue api key")
    .plaintext;

    (tenant.id, workspace.id, owner.id, plaintext)
}

/// Issue an API key for an existing user.
///
/// Used when a test needs multiple keys (e.g. audit_log needs both a
/// migration-scope key and an audit-scope key). Call
/// `create_tenant_workspace_owner` first to get the `owner_id`, then
/// `issue_api_key` for additional keys.
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
