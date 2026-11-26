//! Tenant model: detail and metadata updates, the infrastructure tenant, and the creation and retirement operations in its submodules.

use crate::error::OptionExt;
pub use crate::models::_entities::tenant_tenants::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

mod operations;
pub mod retirement;

pub(crate) use operations::count_tenants;
pub use operations::create_tenant;

/// # Errors
/// Rejects unauthorized credentials or propagates database errors.
pub(crate) async fn detail_as(
    db: &DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
) -> Result<Model, crate::error::YorishiroError> {
    use crate::error::ResultExt;
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        crate::models::iam::policy::Action::GetTenant,
        crate::models::iam::policy::Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let tenant = Model::find_by_id(&txn, actor.tenant_id).await?;
    txn.commit().await.internal()?;
    Ok(tenant)
}

/// Updates the current tenant's name and manual workspace cap.
///
/// # Errors
/// Rejects unauthorized credentials, invalid names or caps and propagates persistence errors.
pub(crate) async fn update_as(
    db: &DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    input: crate::dtos::tenants::UpdateTenantRequest,
) -> Result<Model, crate::error::YorishiroError> {
    use crate::error::{ResultExt, YorishiroError};
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        crate::models::iam::policy::Action::UpdateTenant,
        crate::models::iam::policy::Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    if input.max_workspaces.is_some() && !input.replace_max_workspaces {
        return Err(YorishiroError::Conflict {
            message: "max_workspaces requires replace_max_workspaces=true".into(),
        });
    }
    if input.max_workspaces.is_some_and(|limit| limit < 0) {
        return Err(YorishiroError::Conflict {
            message: "max_workspaces must be non-negative".into(),
        });
    }
    let mut tenant: ActiveModel = Model::find_by_id(&txn, actor.tenant_id).await?.into();
    if let Some(name) = input.name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(YorishiroError::Conflict {
                message: "name must be between 1 and 100 characters".into(),
            });
        }
        tenant.name = sea_orm::ActiveValue::Set(name.to_owned());
    }
    if input.replace_max_workspaces {
        tenant.max_workspaces = sea_orm::ActiveValue::Set(input.max_workspaces);
    }
    let updated = tenant.update(&txn).await.internal()?;
    txn.commit().await.internal()?;
    Ok(updated)
}

/// The nil UUID reserved for infrastructure tenants that own no members and no data of their own.
pub(crate) const INFRASTRUCTURE_TENANT_ID: Uuid = Uuid::nil();

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

impl Model {
    /// Rejects credentials and new grants for a disabled or missing tenant.
    ///
    /// # Errors
    /// Returns unauthenticated for unavailable tenants or propagates database failures.
    pub(crate) async fn require_active(
        conn: &impl ConnectionTrait,
        id: Uuid,
    ) -> Result<(), crate::error::YorishiroError> {
        use crate::error::{ResultExt, YorishiroError};
        Entity::find_by_id(id)
            .one(conn)
            .await
            .internal()?
            .filter(|tenant| tenant.disabled_at.is_none())
            .ok_or(YorishiroError::Unauthenticated)?;
        Ok(())
    }

    /// Fetches a tenant by its primary key.
    ///
    /// # Errors
    /// Returns an error if the tenant is missing or the query fails.
    pub async fn find_by_id(
        conn: &impl ConnectionTrait,
        id: Uuid,
    ) -> Result<Self, crate::error::YorishiroError> {
        use crate::error::ResultExt;

        Entity::find_by_id(id)
            .one(conn)
            .await
            .internal()?
            .or_not_found("tenant not found")
    }
}
