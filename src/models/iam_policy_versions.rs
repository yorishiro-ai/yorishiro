//! IAM policy versions: a policy's document only changes by adding a version, and evaluation uses the default one.
//!
//! A policy holds at most five versions.
//! A managed policy exposes one virtual version generated from code, whose id is the policy's own id.

use crate::error::OptionExt;
pub use crate::models::_entities::iam_policy_versions::{ActiveModel, Column, Entity, Model};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, entity::prelude::*};
use serde::Serialize;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_workspaces;
use crate::models::iam::policy::{Action, Policy, Resource};
use crate::models::iam_policies;

pub(crate) const MAX_VERSIONS: u64 = 5;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
    }
}

/// One version of a policy, stored or virtual.
#[derive(Clone, Debug, Serialize)]
pub struct PolicyVersion {
    pub id: Uuid,
    pub policy_id: Uuid,
    pub version_number: i32,
    pub is_default: bool,
    pub document: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

impl From<Model> for PolicyVersion {
    fn from(row: Model) -> Self {
        Self {
            id: row.id,
            policy_id: row.policy_id,
            version_number: row.version_number,
            is_default: row.is_default,
            document: row.document,
            created_at: row.created_at,
        }
    }
}

/// Checks a document against the policy language, including that every workspace it names belongs to the tenant.
///
/// # Errors
/// Returns a validation error for a malformed document or a foreign workspace.
pub(crate) async fn validate_document(
    txn: &sea_orm::DatabaseTransaction,
    tenant_id: Uuid,
    document: &serde_json::Value,
) -> Result<(), YorishiroError> {
    let json = serde_json::to_string(document).internal()?;
    let ids = Policy::workspace_ids(&json)?;
    let owned = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .filter(workspace_workspaces::Column::Id.is_in(ids))
        .all(txn)
        .await
        .internal()?;
    Policy::parse(&json, tenant_id, |tenant, workspace| {
        tenant == tenant_id && owned.iter().any(|w| w.id == workspace)
    })?;
    Ok(())
}

impl Model {
    /// The default version of each policy in `policy_ids`.
    pub(crate) async fn defaults(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        policy_ids: Vec<Uuid>,
    ) -> Result<Vec<Self>, YorishiroError> {
        Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::PolicyId.is_in(policy_ids))
            .filter(Column::IsDefault.eq(true))
            .all(conn)
            .await
            .internal()
    }

    pub(crate) async fn count(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        policy_id: Uuid,
    ) -> Result<u64, YorishiroError> {
        Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::PolicyId.eq(policy_id))
            .count(conn)
            .await
            .internal()
    }

    /// Adds a version to a policy the caller has already locked and checked.
    pub(crate) async fn add(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        policy_id: Uuid,
        document: serde_json::Value,
        make_default: bool,
    ) -> Result<Self, YorishiroError> {
        validate_document(txn, tenant_id, &document).await?;
        if Self::count(txn, tenant_id, policy_id).await? >= MAX_VERSIONS {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "a policy can have at most {MAX_VERSIONS} versions; delete one first"
                ),
            });
        }
        let latest = Entity::find()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::PolicyId.eq(policy_id))
            .order_by_desc(Column::VersionNumber)
            .one(txn)
            .await
            .internal()?
            .map_or(0, |row| row.version_number);
        if make_default {
            Self::clear_default(txn, tenant_id, policy_id).await?;
        }
        ActiveModel {
            tenant_id: Set(tenant_id),
            policy_id: Set(policy_id),
            version_number: Set(latest + 1),
            document: Set(document),
            is_default: Set(make_default),
            ..Default::default()
        }
        .insert(txn)
        .await
        .internal()
    }

    async fn clear_default(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        policy_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use sea_orm::sea_query::Expr;
        Entity::update_many()
            .col_expr(Column::IsDefault, Expr::value(false))
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::PolicyId.eq(policy_id))
            .filter(Column::IsDefault.eq(true))
            .exec(txn)
            .await
            .internal()?;
        Ok(())
    }

    async fn require_stored_policy(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        policy_id: Uuid,
    ) -> Result<iam_policies::Model, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        let policy = iam_policies::Model::find(txn, tenant_id, policy_id)
            .await?
            .or_not_found("IAM policy was not found")?;
        if crate::models::iam::builtin::is_managed_policy_name(&policy.name) {
            return Err(YorishiroError::Conflict {
                message: "managed policies have a single version generated by the product".into(),
            });
        }
        Ok(policy)
    }

    pub(crate) async fn create_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        policy_id: Uuid,
        document: serde_json::Value,
        set_as_default: bool,
    ) -> Result<PolicyVersion, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::CreatePolicyVersion,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = Self::require_stored_policy(&txn, actor.tenant_id, policy_id).await?;
        let version = Self::add(&txn, actor.tenant_id, policy_id, document, set_as_default).await?;
        crate::models::api_events::record(
            &txn,
            actor,
            Action::CreatePolicyVersion,
            serde_json::json!({
                "policy_id": policy_id,
                "policy_name": policy.name,
                "version_id": version.id,
                "version_number": version.version_number,
                "set_as_default": set_as_default,
                "document": version.document,
            }),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(version.into())
    }

    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        policy_id: Uuid,
    ) -> Result<Vec<PolicyVersion>, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::ListPolicyVersions,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .or_not_found("IAM policy was not found")?;
        let versions = if let Some(virtual_version) = virtual_version(&policy) {
            vec![virtual_version]
        } else {
            Entity::find()
                .filter(Column::TenantId.eq(actor.tenant_id))
                .filter(Column::PolicyId.eq(policy_id))
                .order_by_asc(Column::VersionNumber)
                .all(&txn)
                .await
                .internal()?
                .into_iter()
                .map(PolicyVersion::from)
                .collect()
        };
        txn.commit().await.internal()?;
        Ok(versions)
    }

    pub(crate) async fn get_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        policy_id: Uuid,
        version_id: Uuid,
    ) -> Result<PolicyVersion, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::GetPolicyVersion,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = iam_policies::Model::find(&txn, actor.tenant_id, policy_id)
            .await?
            .or_not_found("IAM policy was not found")?;
        let version = if let Some(virtual_version) = virtual_version(&policy) {
            (virtual_version.id == version_id).then_some(virtual_version)
        } else {
            Self::find(&txn, actor.tenant_id, policy_id, version_id)
                .await?
                .map(PolicyVersion::from)
        }
        .or_not_found("IAM policy version was not found")?;
        txn.commit().await.internal()?;
        Ok(version)
    }

    pub(crate) async fn set_default_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        policy_id: Uuid,
        version_id: Uuid,
    ) -> Result<PolicyVersion, YorishiroError> {
        use sea_orm::sea_query::Expr;
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::SetDefaultPolicyVersion,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = Self::require_stored_policy(&txn, actor.tenant_id, policy_id).await?;
        Self::find(&txn, actor.tenant_id, policy_id, version_id)
            .await?
            .or_not_found("IAM policy version was not found")?;
        Self::clear_default(&txn, actor.tenant_id, policy_id).await?;
        Entity::update_many()
            .col_expr(Column::IsDefault, Expr::value(true))
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::Id.eq(version_id))
            .exec(&txn)
            .await
            .internal()?;
        let version = Self::find(&txn, actor.tenant_id, policy_id, version_id)
            .await?
            .or_not_found("IAM policy version was not found")?;
        crate::models::api_events::record(
            &txn,
            actor,
            Action::SetDefaultPolicyVersion,
            serde_json::json!({
                "policy_id": policy_id,
                "policy_name": policy.name,
                "version_id": version_id,
                "version_number": version.version_number,
            }),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(version.into())
    }

    pub(crate) async fn delete_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        policy_id: Uuid,
        version_id: Uuid,
    ) -> Result<(), YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::DeletePolicyVersion,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = Self::require_stored_policy(&txn, actor.tenant_id, policy_id).await?;
        let version = Self::find(&txn, actor.tenant_id, policy_id, version_id)
            .await?
            .or_not_found("IAM policy version was not found")?;
        if version.is_default {
            return Err(YorishiroError::Conflict {
                message: "the default policy version cannot be deleted; set another version as the default first".into(),
            });
        }
        Entity::delete_by_id(version_id)
            .exec(&txn)
            .await
            .internal()?;
        crate::models::api_events::record(
            &txn,
            actor,
            Action::DeletePolicyVersion,
            serde_json::json!({
                "policy_id": policy_id,
                "policy_name": policy.name,
                "version_id": version_id,
                "version_number": version.version_number,
            }),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    async fn find(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        policy_id: Uuid,
        version_id: Uuid,
    ) -> Result<Option<Self>, YorishiroError> {
        Entity::find_by_id(version_id)
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::PolicyId.eq(policy_id))
            .one(conn)
            .await
            .internal()
    }
}

/// The single version a managed policy exposes, or `None` for a tenant-owned policy.
pub(crate) fn virtual_version(policy: &iam_policies::Model) -> Option<PolicyVersion> {
    let document = crate::models::iam::builtin::managed_document(&policy.name)?;
    Some(PolicyVersion {
        id: policy.id,
        policy_id: policy.id,
        version_number: 1,
        is_default: true,
        document,
        created_at: policy.created_at,
    })
}
