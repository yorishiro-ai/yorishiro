//! Billing state stored in this crate's own `identity_tenant_billing` table.
//!
//! `yorishiro-core` owns `identity_tenants` and knows nothing about subscriptions or payment processors, so the plan and the Stripe customer id live here instead, keyed by tenant id.
//! A tenant with no row is unbilled (the state every self-hosted deployment is permanently in), which is why every read returns an `Option` rather than treating a missing row as an error.

use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};

/// A tenant's billing state. Absent for any tenant that has never been through checkout.
#[derive(Debug, Clone, FromQueryResult)]
pub struct TenantBillingRecord {
    pub tenant_id: Uuid,
    pub plan: Option<String>,
    pub stripe_customer_id: Option<String>,
}

/// Reads a tenant's billing state.
/// `None` means the tenant is unbilled, not that it is missing: the caller decides what an
/// unbilled tenant looks like (the dashboard renders it as no plan and no cap).
pub async fn get_billing(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    TenantBillingRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT tenant_id, plan, stripe_customer_id FROM identity_tenant_billing \
         WHERE tenant_id = $1",
        [tenant_id.into()],
    ))
    .one(conn)
    .await
    .internal()
}

/// Resolves the tenant a Stripe webhook is about.
/// Subscription updated/deleted events carry only the Stripe customer id, so this is the inbound
/// lookup path.
pub async fn get_by_stripe_customer(
    conn: &impl ConnectionTrait,
    stripe_customer_id: &str,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    TenantBillingRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT tenant_id, plan, stripe_customer_id FROM identity_tenant_billing \
         WHERE stripe_customer_id = $1",
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
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_tenant_billing (tenant_id, stripe_customer_id) VALUES ($1, $2) \
         ON CONFLICT (tenant_id) DO UPDATE \
         SET stripe_customer_id = EXCLUDED.stripe_customer_id, updated_at = now()",
        [tenant_id.into(), stripe_customer_id.into()],
    ))
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
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_tenant_billing (tenant_id, plan) VALUES ($1, $2) \
         ON CONFLICT (tenant_id) DO UPDATE SET plan = EXCLUDED.plan, updated_at = now()",
        [tenant_id.into(), plan.into()],
    ))
    .await
    .internal()?;
    Ok(())
}
