//! System-managed IAM objects that every tenant has, the tenant equivalent of AWS managed policies.
//!
//! A fixed catalogue of tenant-scoped policies is created with the tenant, and the API refuses to create, rename, update or delete the built-ins by name.
//! An administrator is an ordinary tenant user who has the Tenant Administrator policy attached, the way an AWS user carries `AdministratorAccess`.
//! The tenant root is a separate identity and needs no policy; every other user starts with the implicit deny.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ActiveValue::Set, ColumnTrait, DatabaseTransaction, EntityTrait,
    QueryFilter,
};
use serde_json::json;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::iam_policies;
use crate::models::iam_policy::{Action, Policy};

pub(crate) const ADMINISTRATOR_POLICY_NAME: &str = "Tenant Administrator";
pub(crate) const WRITER_POLICY_NAME: &str = "Tenant Writer";
pub(crate) const READER_POLICY_NAME: &str = "Tenant Reader";

/// Names the system reserves: the API refuses to create, rename, update or delete objects under them.
pub(crate) fn is_builtin_policy_name(name: &str) -> bool {
    matches!(
        name,
        ADMINISTRATOR_POLICY_NAME | WRITER_POLICY_NAME | READER_POLICY_NAME
    )
}

/// Read classification is name-based on purpose: an added action defaults to write and must be named to join the reader policy.
fn is_read_action(action: Action) -> bool {
    let value =
        serde_json::to_value(action).expect("actions serialize to their snake_case wire name");
    let name = value.as_str().expect("the action name is a string");
    name.ends_with("_get")
        || name.ends_with("_list")
        || name.ends_with("_preview")
        || matches!(name, "search" | "recall" | "export" | "migration_dry_run")
}

/// Identity and permission management the writer policy denies: a writer manages content, not the tenant's people, groups, policies or credentials.
fn is_admin_only_action(action: Action) -> bool {
    matches!(
        action,
        Action::TenantUpdate
            | Action::CredentialList
            | Action::CredentialCreate
            | Action::CredentialRotate
            | Action::CredentialRevoke
            | Action::TenantUserList
            | Action::TenantUserCreate
            | Action::TenantUserUpdate
            | Action::TenantUserDelete
            | Action::TenantUserPasswordReset
            | Action::GroupList
            | Action::GroupGet
            | Action::GroupCreate
            | Action::GroupUpdate
            | Action::GroupDelete
            | Action::GroupMemberList
            | Action::GroupMemberAdd
            | Action::GroupMemberRemove
            | Action::PolicyList
            | Action::PolicyGet
            | Action::PolicyCreate
            | Action::PolicyUpdate
            | Action::PolicyDelete
            | Action::UserPolicyList
            | Action::GroupPolicyList
            | Action::UserPolicyAttach
            | Action::UserPolicyDetach
            | Action::GroupPolicyAttach
            | Action::GroupPolicyDetach
            | Action::IamAuditList
            | Action::RoleList
            | Action::RoleGet
            | Action::RoleCreate
            | Action::RoleUpdate
            | Action::RoleDelete
            | Action::RolePolicyList
            | Action::RolePolicyAttach
            | Action::RolePolicyDetach
    )
}

fn action_values(actions: impl IntoIterator<Item = Action>) -> Vec<serde_json::Value> {
    actions
        .into_iter()
        .map(|action| serde_json::to_value(action).expect("actions serialize to strings"))
        .collect()
}

/// Statements for one effect and resource set, chunked to the parser's per-statement action cap.
fn statements(
    effect: &str,
    actions: Vec<serde_json::Value>,
    resources: serde_json::Value,
) -> Vec<serde_json::Value> {
    actions
        .chunks(crate::models::iam_policy::MAX_ACTIONS)
        .map(|chunk| json!({"effect": effect, "actions": chunk, "resources": resources.clone()}))
        .collect()
}

/// The system-managed policy documents for a tenant, in a stable order.
pub(crate) fn policy_documents(tenant_id: Uuid) -> Vec<(String, serde_json::Value)> {
    let tenant = json!({"type": "tenant", "tenant_id": tenant_id});
    let all_workspaces = json!({"type": "all_workspaces", "tenant_id": tenant_id});
    let tenant_resources = json!([tenant]);
    let all_roles = json!({"type": "all_roles", "tenant_id": tenant_id});
    let both_resources = json!([tenant, all_workspaces, all_roles]);
    let wildcard = json!([json!("*")]);

    let reads = Action::ALL
        .iter()
        .copied()
        .filter(|action| is_read_action(*action))
        .collect::<Vec<_>>();
    let tenant_reads = reads
        .iter()
        .filter(|action| {
            action.applies_to(crate::models::iam_policy::Resource::Tenant { tenant_id })
        })
        .cloned()
        .collect::<Vec<Action>>();
    let workspace_reads = reads
        .iter()
        .filter(|action| {
            action.applies_to(crate::models::iam_policy::Resource::AllWorkspaces { tenant_id })
        })
        .cloned()
        .collect::<Vec<Action>>();
    let admin_only = Action::ALL
        .iter()
        .copied()
        .filter(|action| is_admin_only_action(*action))
        .collect::<Vec<_>>();

    let writer_statements = [json!(
        {"effect": "allow", "actions": wildcard, "resources": both_resources}
    )]
    .into_iter()
    .chain(statements(
        "deny",
        action_values(admin_only),
        tenant_resources.clone(),
    ))
    .collect::<Vec<_>>();
    let reader_statements = statements("allow", action_values(tenant_reads), tenant_resources)
        .into_iter()
        .chain(statements(
            "allow",
            action_values(workspace_reads),
            json!([all_workspaces]),
        ))
        .collect::<Vec<_>>();

    vec![
        (
            ADMINISTRATOR_POLICY_NAME.to_owned(),
            json!({"statements": [json!(
                {"effect": "allow", "actions": wildcard, "resources": both_resources}
            )]}),
        ),
        (
            WRITER_POLICY_NAME.to_owned(),
            json!({"statements": writer_statements}),
        ),
        (
            READER_POLICY_NAME.to_owned(),
            json!({"statements": reader_statements}),
        ),
    ]
}

async fn find_policy(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    name: &str,
) -> Result<Option<iam_policies::Model>, YorishiroError> {
    iam_policies::Entity::find()
        .filter(iam_policies::Column::TenantId.eq(tenant_id))
        .filter(iam_policies::Column::Name.eq(name))
        .one(txn)
        .await
        .internal()
}

/// Creates the system-managed policies for a tenant.
/// Idempotent: rows that already exist are left untouched, so every tenant-creating path can call it.
///
/// # Errors
/// Returns a validation error when a generated document does not parse, or a persistence failure.
pub(crate) async fn bootstrap(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    for (name, document) in policy_documents(tenant_id) {
        let rendered = serde_json::to_string(&document).internal()?;
        // Built-in documents never name concrete workspaces, so tenant ownership is the whole check.
        Policy::parse(&rendered, tenant_id, |tenant, _| tenant == tenant_id)?;
        if find_policy(txn, tenant_id, &name).await?.is_some() {
            continue;
        }
        iam_policies::ActiveModel {
            id: crate::db::sqlite_generated_id(txn, ActiveValue::NotSet),
            tenant_id: Set(tenant_id),
            name: Set(name),
            document: Set(document),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()?;
    }
    Ok(())
}
