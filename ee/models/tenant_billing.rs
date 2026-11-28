//! Billing state stored in this crate's own `tenant_billing` table.
//!
//! The plan and the Stripe customer id live here, keyed by tenant id, because `models::tenant_tenants` knows nothing about billing.
//! A tenant with no row is unbilled, so reads return an `Option` rather than an error.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::tenant_billing::{ActiveModel, Column, Entity};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

/// A tenant's billing state.
/// Absent for any tenant that has never been through checkout.
#[derive(Clone)]
pub struct TenantBillingRecord {
    pub(crate) tenant_id: Uuid,
    pub plan: Option<String>,
    pub stripe_customer_id: Option<String>,
}

impl From<crate::models::_entities::tenant_billing::Model> for TenantBillingRecord {
    fn from(model: crate::models::_entities::tenant_billing::Model) -> Self {
        Self {
            tenant_id: model.tenant_id,
            plan: model.plan,
            stripe_customer_id: model.stripe_customer_id,
        }
    }
}

/// Reads a tenant's billing state.
/// `None` means the tenant is unbilled, not that it is missing.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn get_billing(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    Entity::find()
        .filter(Column::TenantId.eq(tenant_id))
        .one(conn)
        .await
        .internal()
        .map(|row| row.map(TenantBillingRecord::from))
}

/// Resolves the tenant a Stripe webhook is about, since subscription events carry only the customer id.
pub(crate) async fn get_by_stripe_customer(
    conn: &impl ConnectionTrait,
    stripe_customer_id: &str,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    Entity::find()
        .filter(Column::StripeCustomerId.eq(stripe_customer_id))
        .one(conn)
        .await
        .internal()
        .map(|row| row.map(TenantBillingRecord::from))
}

/// Records the Stripe customer id created for a tenant at checkout.
/// Upserts, because a resubscribe reaches checkout with an existing row.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn link_stripe_customer(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    stripe_customer_id: &str,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        stripe_customer_id: ActiveValue::Set(Some(stripe_customer_id.to_string())),
        updated_at: ActiveValue::Set(crate::data::time::now().into()),
        ..Default::default()
    };
    // `Entity::insert(..).on_conflict(..)` skips `before_save`, so the hook runs by hand to stamp timestamps.
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::TenantId)
                .update_columns([Column::StripeCustomerId, Column::UpdatedAt])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Sets a tenant's plan.
/// Upserts because the plan can land before or after the customer id is linked.
/// The workspace cap lives on `tenant_tenants.max_workspaces`, so the caller applies it separately.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn set_plan(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    plan: &str,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        plan: ActiveValue::Set(Some(plan.to_string())),
        updated_at: ActiveValue::Set(crate::data::time::now().into()),
        ..Default::default()
    };
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::TenantId)
                .update_columns([Column::Plan, Column::UpdatedAt])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
