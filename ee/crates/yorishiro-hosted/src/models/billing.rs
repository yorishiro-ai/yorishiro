//! Billing state stored in this crate's own `identity_tenant_billing` table.
//!
//! `yorishiro-core` owns `identity_tenants` and knows nothing about subscriptions or payment processors, so the plan and the Stripe customer id live here instead, keyed by tenant id.
//! A tenant with no row is unbilled (the state every self-hosted deployment is permanently in), which is why every read returns an `Option` rather than treating a missing row as an error.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ConnectionTrait, EntityTrait, FromQueryResult, Statement};
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::identity_tenant_billing::{ActiveModel, Column, Entity};

/// A tenant's billing state. Absent for any tenant that has never been through checkout.
#[derive(Debug, Clone, FromQueryResult)]
pub struct TenantBillingRecord {
    pub tenant_id: Uuid,
    pub plan: Option<String>,
    pub stripe_customer_id: Option<String>,
}

/// The columns [`TenantBillingRecord`] needs, shared by both lookups below so they can't drift
/// apart from each other (see `search.rs`'s `HIT_COLUMNS` for the same pattern).
const BILLING_COLUMNS: &str = "tenant_id, plan, stripe_customer_id";

/// Reads a tenant's billing state.
/// `None` means the tenant is unbilled, not that it is missing: the caller decides what an unbilled tenant looks like (the dashboard renders it as no plan and no cap).
pub async fn get_billing(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    TenantBillingRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        format!("SELECT {BILLING_COLUMNS} FROM identity_tenant_billing WHERE tenant_id = $1"),
        [tenant_id.into()],
    ))
    .one(conn)
    .await
    .internal()
}

/// Resolves the tenant a Stripe webhook is about.
/// Subscription updated/deleted events carry only the Stripe customer id, so this is the inbound lookup path.
pub async fn get_by_stripe_customer(
    conn: &impl ConnectionTrait,
    stripe_customer_id: &str,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    TenantBillingRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        format!(
            "SELECT {BILLING_COLUMNS} FROM identity_tenant_billing WHERE stripe_customer_id = $1"
        ),
        [stripe_customer_id.into()],
    ))
    .one(conn)
    .await
    .internal()
}

/// Records the Stripe customer id created for a tenant at checkout, so later webhook events can be routed back to it via [`get_by_stripe_customer`].
/// Upserts, because checkout can be completed for a tenant that already has a billing row (a resubscribe after cancellation).
pub async fn link_stripe_customer(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    stripe_customer_id: &str,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        stripe_customer_id: ActiveValue::Set(Some(stripe_customer_id.to_string())),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    // updated_at is set explicitly here rather than left to ActiveModelBehavior::before_save:
    // Entity::insert(...).on_conflict(...) builds and executes a raw INSERT ... ON CONFLICT
    // statement directly, bypassing before_save entirely (it only runs on the
    // ActiveModelTrait::insert/update/save path), for both the insert and the conflict-update
    // branch.
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
/// Upserts for the same reason as [`link_stripe_customer`]: a plan can be assigned before or after the customer id is linked, depending on which webhook lands first.
///
/// The workspace cap that comes with the plan is not written here: it lives on `identity_tenants.max_workspaces`, which base owns and enforces at workspace-creation time.
/// The caller applies both.
pub async fn set_plan(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    plan: &str,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        plan: ActiveValue::Set(Some(plan.to_string())),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
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
