//! Tenant creation, limits, and tenant-owned settings.

use super::{ActiveModel, Entity, INFRASTRUCTURE_TENANT_ID, Model};
use crate::data::tenant_policy::TenantLimit;
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::tenant_tenants::Column;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter,
};

/// Counts real (non-infrastructure) tenants: every row except `INFRASTRUCTURE_TENANT_ID`.
pub(crate) async fn count_tenants(conn: &impl ConnectionTrait) -> Result<u64, YorishiroError> {
    Entity::find()
        .filter(Column::Id.ne(INFRASTRUCTURE_TENANT_ID))
        .count(conn)
        .await
        .internal()
}

/// Creates a tenant, enforcing a tenant cap against `count_tenants`.
/// The transaction keeps the count and insert under the same lock until commit.
/// Backend isolation constraints and deployment quotas are resolved together by `TenantLimit`.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn create_tenant(
    conn: &DatabaseTransaction,
    name: &str,
    effective_max: Option<i32>,
) -> Result<Model, YorishiroError> {
    let limit = TenantLimit::resolve(conn.get_database_backend(), effective_max);
    if let Some(max) = limit.maximum() {
        crate::db::lock_for_update(conn, "create_tenant")
            .await
            .internal()?;
        let count = count_tenants(conn).await?;
        if count >= max {
            return Err(limit.exhausted());
        }
    }

    let active = ActiveModel {
        name: ActiveValue::Set(name.to_string()),
        max_workspaces: ActiveValue::Set(None),
        ..Default::default()
    };
    // `id` on SQLite is filled in by tenant_tenants::ActiveModel's before_save (crate::db::sqlite_generated_id), not here.
    active.insert(conn).await.internal()
}
