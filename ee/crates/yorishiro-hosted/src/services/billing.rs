//! Billing state stored in this repository's own `identity.tenant_billing` table.
//!
//! `yorishiro-core` owns `identity.tenants` and knows nothing about subscriptions or payment
//! processors, so the plan and the Stripe customer id live here instead, keyed by tenant id.
//! A tenant with no row is unbilled (the state every self-hosted deployment is permanently in),
//! which is why every read returns an `Option` rather than treating a missing row as an error.

use sea_query::{Alias, Expr, Iden, OnConflict, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};

#[derive(Iden)]
enum TenantBilling {
    Table,
    TenantId,
    Plan,
    StripeCustomerId,
    UpdatedAt,
}

fn billing_columns() -> [TenantBilling; 3] {
    [
        TenantBilling::TenantId,
        TenantBilling::Plan,
        TenantBilling::StripeCustomerId,
    ]
}

/// A tenant's billing state. Absent for any tenant that has never been through checkout.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TenantBillingRecord {
    pub tenant_id: Uuid,
    pub plan: Option<String>,
    pub stripe_customer_id: Option<String>,
}

/// Reads a tenant's billing state. `None` means the tenant is unbilled, not that it is missing:
/// the caller decides what an unbilled tenant looks like (the dashboard renders it as no plan and
/// no cap).
pub async fn get_billing(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(billing_columns())
        .from((Alias::new("identity"), TenantBilling::Table))
        .and_where(Expr::col(TenantBilling::TenantId).eq(tenant_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantBillingRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()
}

/// Resolves the tenant a Stripe webhook is about. Subscription updated/deleted events carry only
/// the Stripe customer id, so this is the inbound lookup path.
pub async fn get_by_stripe_customer(
    pool: &PgPool,
    stripe_customer_id: &str,
) -> Result<Option<TenantBillingRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(billing_columns())
        .from((Alias::new("identity"), TenantBilling::Table))
        .and_where(Expr::col(TenantBilling::StripeCustomerId).eq(stripe_customer_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantBillingRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()
}

/// Records the Stripe customer id created for a tenant at checkout, so later webhook events can
/// be routed back to it via [`get_by_stripe_customer`]. Upserts, because checkout can be
/// completed for a tenant that already has a billing row (a resubscribe after cancellation).
pub async fn link_stripe_customer(
    pool: &PgPool,
    tenant_id: Uuid,
    stripe_customer_id: &str,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), TenantBilling::Table))
        .columns([TenantBilling::TenantId, TenantBilling::StripeCustomerId])
        .values_panic([tenant_id.into(), stripe_customer_id.into()])
        .on_conflict(
            OnConflict::column(TenantBilling::TenantId)
                .values([
                    (TenantBilling::StripeCustomerId, stripe_customer_id.into()),
                    (TenantBilling::UpdatedAt, Expr::current_timestamp().into()),
                ])
                .to_owned(),
        )
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}

/// Sets a tenant's plan. Upserts for the same reason as [`link_stripe_customer`]: a plan can be
/// assigned before or after the customer id is linked, depending on which webhook lands first.
///
/// The workspace cap that comes with the plan is not written here: it lives on
/// `identity.tenants.max_workspaces`, which the community edition owns and enforces at
/// workspace-creation time. The caller applies both.
pub async fn set_plan(pool: &PgPool, tenant_id: Uuid, plan: &str) -> Result<(), YorishiroError> {
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), TenantBilling::Table))
        .columns([TenantBilling::TenantId, TenantBilling::Plan])
        .values_panic([tenant_id.into(), plan.into()])
        .on_conflict(
            OnConflict::column(TenantBilling::TenantId)
                .values([
                    (TenantBilling::Plan, plan.into()),
                    (TenantBilling::UpdatedAt, Expr::current_timestamp().into()),
                ])
                .to_owned(),
        )
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/services/billing.rs"]
mod tests;
