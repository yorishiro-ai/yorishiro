pub use crate::models::_entities::tenant_tenants::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

mod operations;
pub mod retirement;

pub(crate) use operations::count_tenants;
pub use operations::create_tenant;

/// Reads the authenticated tenant's administrator-visible metadata.
///
/// # Errors
/// Rejects stale credentials and non-administrators or propagates database errors.
pub(crate) async fn detail_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
) -> Result<Model, crate::error::YorishiroError> {
    let current = crate::models::api_keys::Model::current_context(db, actor).await?;
    crate::models::access::require_tenant_admin(db, current.tenant_id, current.user_id).await?;
    Model::find_by_id(db, current.tenant_id).await
}

/// Updates current-tenant metadata under the membership fence without changing plan quotas.
///
/// # Errors
/// Rejects non-administrators, invalid names or caps and propagates persistence errors.
pub(crate) async fn update_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    input: crate::dtos::tenants::UpdateTenantRequest,
) -> Result<Model, crate::error::YorishiroError> {
    use crate::error::{ResultExt, YorishiroError};
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::access::require_current_tenant_admin(&txn, actor).await?;
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
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
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
        use crate::error::{ResultExt, YorishiroError};

        Entity::find_by_id(id)
            .one(conn)
            .await
            .internal()?
            .ok_or_else(|| YorishiroError::not_found("tenant not found"))
    }
}

pub(crate) async fn list_all(
    conn: &impl ConnectionTrait,
) -> Result<Vec<Model>, crate::error::YorishiroError> {
    use crate::error::ResultExt;

    Entity::find().all(conn).await.internal()
}
