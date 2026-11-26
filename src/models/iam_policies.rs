//! IAM policy model: creating, reading and deleting policies, and resolving the policies that apply to a user.
//! A policy's document lives in its versions; see `iam_policy_versions`.

use crate::error::OptionExt;
pub use crate::models::_entities::iam_policies::{ActiveModel, Column, Entity, Model};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};
use crate::models::iam::policy::{Action, Policy, Resource};
use crate::models::iam_policy_versions;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        this.updated_at = crate::db::stamped_updated_at(this.updated_at);
        Ok(this)
    }
}

/// A policy with the document of its default version, as shown to callers.
#[derive(Clone, Debug)]
pub struct PolicyView {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub yrn: String,
    pub managed: bool,
    pub default_version_id: Uuid,
    pub document: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
}

/// A policy attached to a user or a group.
#[derive(Clone, Debug)]
pub struct AttachedPolicy {
    pub policy_id: Uuid,
    pub name: String,
    pub yrn: String,
}

/// The document that decides requests for one policy a user holds.
#[derive(Clone, Debug)]
pub(crate) struct EffectivePolicy {
    pub(crate) name: String,
    document: serde_json::Value,
}

impl EffectivePolicy {
    /// Parses the document, using the one generated from code for a managed policy.
    pub(crate) fn parse(&self, tenant_id: Uuid) -> Result<Policy, YorishiroError> {
        if let Some(managed) = crate::models::iam::builtin::managed_policy(&self.name, tenant_id)? {
            return Ok(managed);
        }
        let json = serde_json::to_string(&self.document).internal()?;
        // Ownership is verified for the concrete target; stale unrelated selectors must not block it.
        Policy::parse(&json, tenant_id, |tenant, _| tenant == tenant_id)
    }
}

impl Model {
    /// Pairs each policy with its default document.
    pub(crate) async fn views(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        rows: Vec<Self>,
    ) -> Result<Vec<PolicyView>, YorishiroError> {
        let defaults = iam_policy_versions::Model::defaults(
            conn,
            tenant_id,
            rows.iter().map(|row| row.id).collect(),
        )
        .await?;
        rows.into_iter()
            .map(|row| {
                let managed = crate::models::iam::builtin::is_managed_policy_name(&row.name);
                let (default_version_id, document) =
                    if let Some(version) = iam_policy_versions::virtual_version(&row) {
                        (version.id, version.document)
                    } else {
                        let version = defaults
                            .iter()
                            .find(|version| version.policy_id == row.id)
                            .ok_or_else(|| {
                                YorishiroError::Internal("policy has no default version".into())
                            })?;
                        (version.id, version.document.clone())
                    };
                Ok(PolicyView {
                    id: row.id,
                    tenant_id: row.tenant_id,
                    yrn: crate::models::iam::builtin::policy_yrn(row.tenant_id, &row.name),
                    name: row.name,
                    managed,
                    default_version_id,
                    document,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                })
            })
            .collect()
    }

    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<PolicyView>, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::ListPolicies,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        // Managed policies come first, then the tenant's own by name.
        let managed: Vec<Self> = crate::models::iam::builtin::MANAGED_POLICY_NAMES
            .iter()
            .filter_map(|name| {
                crate::models::iam::builtin::managed_policy_id(name)
                    .map(|id| Self::managed(actor.tenant_id, id, name))
            })
            .collect();
        let (offset, limit) = (params.offset(), params.limit());
        let skipped = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(managed.len());
        let taken = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut rows: Vec<Self> = managed.into_iter().skip(skipped).take(taken).collect();
        let remaining = limit.saturating_sub(rows.len() as u64);
        if remaining > 0 {
            rows.extend(
                Entity::find()
                    .filter(Column::TenantId.eq(actor.tenant_id))
                    .order_by_asc(Column::Name)
                    .order_by_asc(Column::Id)
                    .limit(remaining)
                    .offset(offset.saturating_sub(
                        crate::models::iam::builtin::MANAGED_POLICY_NAMES.len() as u64,
                    ))
                    .all(&txn)
                    .await
                    .internal()?,
            );
        }
        let views = Self::views(&txn, actor.tenant_id, rows).await?;
        txn.commit().await.internal()?;
        Ok(views)
    }

    pub(crate) async fn get_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        id: Uuid,
    ) -> Result<PolicyView, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::GetPolicy,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let row = Self::find(&txn, actor.tenant_id, id)
            .await?
            .or_not_found("IAM policy was not found")?;
        let view = Self::views(&txn, actor.tenant_id, vec![row])
            .await?
            .remove(0);
        txn.commit().await.internal()?;
        Ok(view)
    }

    pub(crate) async fn delete_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::{
            iam_group_policy_attachments as groups, iam_user_policy_attachments as users,
        };

        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::DeletePolicy,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let policy = Self::find(&txn, actor.tenant_id, id)
            .await?
            .or_not_found("IAM policy was not found")?;
        if crate::models::iam::builtin::is_managed_policy_name(&policy.name) {
            return Err(YorishiroError::Conflict {
                message: "managed policies cannot be deleted".into(),
            });
        }
        if users::Entity::find()
            .filter(users::Column::TenantId.eq(actor.tenant_id))
            .filter(users::Column::PolicyId.eq(id))
            .exists(&txn)
            .await
            .internal()?
            || groups::Entity::find()
                .filter(groups::Column::TenantId.eq(actor.tenant_id))
                .filter(groups::Column::PolicyId.eq(id))
                .exists(&txn)
                .await
                .internal()?
        {
            return Err(YorishiroError::Conflict {
                message: "detach the policy from users and groups before deletion".into(),
            });
        }
        Self::delete(&txn, actor.tenant_id, id).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::DeletePolicy,
            serde_json::json!({"policy_id": id, "name": policy.name}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn create_as(
        db: &DatabaseConnection,
        actor: &crate::models::access_keys::AuthContext,
        name: String,
        document: serde_json::Value,
    ) -> Result<PolicyView, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::iam::access::require_iam_action(
            &txn,
            actor,
            Action::CreatePolicy,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        crate::models::iam::names::validate_name(
            "policy",
            &name,
            crate::models::iam::names::MAX_POLICY_NAME_CHARS,
        )?;
        if crate::models::iam::builtin::is_managed_policy_name(&name) {
            return Err(YorishiroError::Conflict {
                message: "this policy name is reserved for a managed policy".into(),
            });
        }
        let policy = Self::create(&txn, actor.tenant_id, name, document.clone()).await?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            Action::CreatePolicy,
            serde_json::json!({"policy_id": policy.id, "name": policy.name, "document": document}),
        )
        .await?;
        let view = Self::views(&txn, actor.tenant_id, vec![policy])
            .await?
            .remove(0);
        txn.commit().await.internal()?;
        Ok(view)
    }

    /// Names the policies in `ids`, keeping their order.
    pub(crate) async fn attached(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        ids: Vec<Uuid>,
    ) -> Result<Vec<AttachedPolicy>, YorishiroError> {
        let rows = Self::stored_or_managed(conn, tenant_id, &ids).await?;
        Ok(ids
            .iter()
            .filter_map(|id| rows.iter().find(|row| row.id == *id))
            .map(|row| AttachedPolicy {
                policy_id: row.id,
                yrn: crate::models::iam::builtin::policy_yrn(tenant_id, &row.name),
                name: row.name.clone(),
            })
            .collect())
    }

    /// The policies with these ids: stored ones from the tenant and managed ones from the code.
    async fn stored_or_managed(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        ids: &[Uuid],
    ) -> Result<Vec<Self>, YorishiroError> {
        let mut rows: Vec<Self> = ids
            .iter()
            .filter_map(|id| {
                crate::models::iam::builtin::managed_policy_name(*id)
                    .map(|name| Self::managed(tenant_id, *id, name))
            })
            .collect();
        rows.extend(
            Entity::find()
                .filter(Column::TenantId.eq(tenant_id))
                .filter(Column::Id.is_in(ids.iter().copied()))
                .all(conn)
                .await
                .internal()?,
        );
        Ok(rows)
    }

    /// Whether the policy is the managed `AdministratorAccess`.
    pub(crate) fn is_administrator_access(&self) -> bool {
        self.name == crate::models::iam::builtin::ADMINISTRATOR_ACCESS
    }

    /// Every policy that applies to a user, from direct attachments and from group memberships, with the document that decides requests.
    pub(crate) async fn effective_for_user(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<EffectivePolicy>, YorishiroError> {
        let rows = Self::for_user(conn, tenant_id, user_id).await?;
        let views = Self::views(conn, tenant_id, rows).await?;
        Ok(views
            .into_iter()
            .map(|view| EffectivePolicy {
                name: view.name,
                document: view.document,
            })
            .collect())
    }

    pub(crate) async fn for_user(
        txn: &impl ConnectionTrait,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::_entities::{
            iam_group_memberships, iam_group_policy_attachments, iam_user_policy_attachments,
        };

        let direct = iam_user_policy_attachments::Entity::find()
            .filter(iam_user_policy_attachments::Column::TenantId.eq(tenant_id))
            .filter(iam_user_policy_attachments::Column::UserId.eq(user_id))
            .all(txn)
            .await
            .internal()?;
        let groups = iam_group_memberships::Entity::find()
            .filter(iam_group_memberships::Column::TenantId.eq(tenant_id))
            .filter(iam_group_memberships::Column::UserId.eq(user_id))
            .all(txn)
            .await
            .internal()?;
        let attached = iam_group_policy_attachments::Entity::find()
            .filter(iam_group_policy_attachments::Column::TenantId.eq(tenant_id))
            .filter(
                iam_group_policy_attachments::Column::GroupId
                    .is_in(groups.into_iter().map(|group| group.group_id)),
            )
            .all(txn)
            .await
            .internal()?;
        let ids: Vec<Uuid> = direct
            .into_iter()
            .map(|attachment| attachment.policy_id)
            .chain(attached.into_iter().map(|attachment| attachment.policy_id))
            .collect();
        Self::stored_or_managed(txn, tenant_id, &ids).await
    }

    /// Creates a policy whose first version, the default, holds `document`.
    pub(crate) async fn create(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        name: String,
        document: serde_json::Value,
    ) -> Result<Self, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        iam_policy_versions::validate_document(txn, tenant_id, &document).await?;
        let policy = ActiveModel {
            tenant_id: Set(tenant_id),
            name: Set(name),
            ..Default::default()
        }
        .insert(txn)
        .await
        .map_err(|error| {
            YorishiroError::conflict_on_unique(error, "an IAM policy with this name already exists")
        })?;
        iam_policy_versions::Model::add(txn, tenant_id, policy.id, document, true).await?;
        Ok(policy)
    }

    /// A managed policy as a model: it has no row, so its id and name are fixed and its timestamps are the epoch.
    fn managed(tenant_id: Uuid, id: Uuid, name: &str) -> Self {
        let epoch = chrono::DateTime::UNIX_EPOCH.fixed_offset();
        Self {
            id,
            tenant_id,
            name: name.to_owned(),
            created_at: epoch,
            updated_at: epoch,
        }
    }

    /// The tenant's stored policy `id`, or the managed policy with that id.
    pub(crate) async fn find(
        txn: &impl ConnectionTrait,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<Option<Self>, YorishiroError> {
        if let Some(name) = crate::models::iam::builtin::managed_policy_name(id) {
            return Ok(Some(Self::managed(tenant_id, id, name)));
        }
        Entity::find_by_id(id)
            .filter(Column::TenantId.eq(tenant_id))
            .one(txn)
            .await
            .internal()
    }

    pub(crate) async fn delete(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<bool, YorishiroError> {
        crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
            .await
            .internal()?;
        Ok(Entity::delete_many()
            .filter(Column::TenantId.eq(tenant_id))
            .filter(Column::Id.eq(id))
            .exec(txn)
            .await
            .internal()?
            .rows_affected
            != 0)
    }
}
