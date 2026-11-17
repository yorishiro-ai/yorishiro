//! System-managed IAM objects that every tenant has, the tenant equivalent of AWS managed policies.
//!
//! A fixed catalogue of tenant-scoped policies is created with the tenant, and the API refuses to create, rename, update or delete the built-ins by name.
//! The built-in group carries the administrator policy, and membership of that group tracks the `admin` role: an admin acts with full tenant permission the way an AWS user with `AdministratorAccess` does.
//! Members holding the `member` or `viewer` role receive no built-in grant and start with the implicit deny, like a fresh IAM user.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ActiveValue::Set, ColumnTrait, DatabaseTransaction, EntityTrait,
    QueryFilter,
};
use serde_json::json;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{
    iam_group_memberships, iam_group_policy_attachments, iam_groups, iam_policies,
};
use crate::models::iam_policy::{Action, Policy};
use crate::models::tenant_memberships::MembershipRole;

pub(crate) const ADMIN_GROUP_NAME: &str = "admins";
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

pub(crate) fn is_builtin_group_name(name: &str) -> bool {
    name == ADMIN_GROUP_NAME
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
    let both_resources = json!([tenant, all_workspaces]);
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

async fn find_group(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    name: &str,
) -> Result<Option<iam_groups::Model>, YorishiroError> {
    iam_groups::Entity::find()
        .filter(iam_groups::Column::TenantId.eq(tenant_id))
        .filter(iam_groups::Column::Name.eq(name))
        .one(txn)
        .await
        .internal()
}

/// Creates the system-managed policies, the built-in group and its policy attachment for a tenant.
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
    if find_group(txn, tenant_id, ADMIN_GROUP_NAME)
        .await?
        .is_none()
    {
        iam_groups::ActiveModel {
            id: crate::db::sqlite_generated_id(txn, ActiveValue::NotSet),
            tenant_id: Set(tenant_id),
            name: Set(ADMIN_GROUP_NAME.to_owned()),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()?;
    }
    let group = find_group(txn, tenant_id, ADMIN_GROUP_NAME)
        .await?
        .expect("the built-in group was just created or pre-existed");
    let policy = find_policy(txn, tenant_id, ADMINISTRATOR_POLICY_NAME)
        .await?
        .expect("the built-in policy was just created or pre-existed");
    let attached = iam_group_policy_attachments::Entity::find()
        .filter(iam_group_policy_attachments::Column::TenantId.eq(tenant_id))
        .filter(iam_group_policy_attachments::Column::GroupId.eq(group.id))
        .filter(iam_group_policy_attachments::Column::PolicyId.eq(policy.id))
        .one(txn)
        .await
        .internal()?;
    if attached.is_none() {
        iam_group_policy_attachments::ActiveModel {
            id: crate::db::sqlite_generated_id(txn, ActiveValue::NotSet),
            tenant_id: Set(tenant_id),
            group_id: Set(group.id),
            policy_id: Set(policy.id),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()?;
    }
    Ok(())
}

/// Tracks the built-in group membership with the membership role, so the `admin` role and the administrator policy stay together.
/// Self-heals a missing built-in group before writing.
///
/// # Errors
/// Propagates persistence failures.
pub(crate) async fn sync_admin_group(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
    role: Option<MembershipRole>,
) -> Result<(), YorishiroError> {
    let mut group = find_group(txn, tenant_id, ADMIN_GROUP_NAME).await?;
    if group.is_none() {
        bootstrap(txn, tenant_id).await?;
        group = find_group(txn, tenant_id, ADMIN_GROUP_NAME).await?;
    }
    let Some(group) = group else {
        return Err(YorishiroError::Internal(anyhow::anyhow!(
            "built-in group is missing after bootstrap"
        )));
    };
    if role == Some(MembershipRole::Admin) {
        use sea_orm::sea_query::OnConflict;
        let active = iam_group_memberships::ActiveModel {
            id: crate::db::sqlite_generated_id(txn, ActiveValue::NotSet),
            tenant_id: Set(tenant_id),
            group_id: Set(group.id),
            user_id: Set(user_id),
            ..Default::default()
        };
        iam_group_memberships::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    iam_group_memberships::Column::TenantId,
                    iam_group_memberships::Column::GroupId,
                    iam_group_memberships::Column::UserId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(txn)
            .await
            .internal()?;
    } else {
        iam_group_memberships::Entity::delete_many()
            .filter(iam_group_memberships::Column::TenantId.eq(tenant_id))
            .filter(iam_group_memberships::Column::GroupId.eq(group.id))
            .filter(iam_group_memberships::Column::UserId.eq(user_id))
            .exec(txn)
            .await
            .internal()?;
    }
    Ok(())
}
