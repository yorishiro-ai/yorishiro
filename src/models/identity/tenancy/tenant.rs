//! Tenant creation, limits, and tenant-owned settings.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait,
    QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::tenant_tenants;

/// Counts real (non-infrastructure) tenants: every row except `INFRASTRUCTURE_TENANT_ID`.
pub(crate) async fn count_tenants(conn: &impl ConnectionTrait) -> Result<u64, YorishiroError> {
    tenant_tenants::Entity::find()
        .filter(tenant_tenants::Column::Id.ne(super::INFRASTRUCTURE_TENANT_ID))
        .count(conn)
        .await
        .internal()
}

/// Creates a tenant, enforcing a tenant cap against `count_tenants`.
/// On Postgres `conn` must be a transaction: this takes `db::lock_for_update` before counting, to close the TOCTOU gap a bare count-then-insert would leave.
/// On SQLite the lock is a no-op (see `db::lock_for_update`'s doc comment for why that is still race-safe) and the cap is not `YORISHIRO_MAX_TENANTS` but a hardcoded 1.
pub(crate) async fn create_tenant(
    conn: &impl ConnectionTrait,
    name: &str,
    configured_max: Option<i32>,
) -> Result<tenant_tenants::Model, YorishiroError> {
    // SQLite has no database-enforced tenant isolation (no RLS, no roles), so a second tenant on that backend would be a silent isolation break rather than merely an unwanted one.
    // The cap is hardcoded rather than read from YORISHIRO_MAX_TENANTS: an operator raising that variable must not be able to loosen a constraint that exists because the isolation mechanism itself is absent, not because of a configurable policy choice.
    let effective_max = if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        Some(1)
    } else {
        configured_max
    };

    if let Some(max) = effective_max {
        crate::db::lock_for_update(conn, "create_tenant")
            .await
            .internal()?;
        let count = count_tenants(conn).await?;
        if count >= max as u64 {
            let remedy = if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                "SQLite deployments are limited to a single tenant, since this backend has no \
                 database-enforced isolation between tenants; use PostgreSQL for more than one"
                    .to_string()
            } else {
                "raise YORISHIRO_MAX_TENANTS or delete an existing tenant".to_string()
            };
            return Err(YorishiroError::Conflict {
                message: format!("this deployment has reached its tenant limit ({max}); {remedy}"),
            });
        }
    }

    let active = tenant_tenants::ActiveModel {
        name: ActiveValue::Set(name.to_string()),
        max_workspaces: ActiveValue::Set(None),
        ..Default::default()
    };
    // `id` on SQLite is filled in by tenant_tenants::ActiveModel's before_save (crate::db::sqlite_generated_id), not here.
    active.insert(conn).await.internal()
}

/// Sets a tenant's `max_workspaces` cap.
///
/// An enterprise-edition deployment never calls this (its tenants keep whatever cap they were created with, `None` by default): the only caller is `ee/`'s Stripe integration, applying the cap that comes with a plan change.
pub(crate) async fn set_tenant_max_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    max_workspaces: Option<i32>,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::tenant_tenants;

    let tenant = tenant_tenants::Entity::find_by_id(tenant_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))?;

    let mut active: tenant_tenants::ActiveModel = tenant.into();
    active.max_workspaces = ActiveValue::Set(max_workspaces);
    active.update(conn).await.internal()?;
    Ok(())
}
